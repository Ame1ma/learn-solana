#![allow(clippy::arithmetic_side_effects)]
#[cfg(not(any(target_env = "msvc", target_os = "freebsd")))]

//! 起点
use jemallocator::Jemalloc;
use {
    agave_validator::{
        admin_rpc_service,
        admin_rpc_service::{load_staked_nodes_overrides, StakedNodesOverrides},
        bootstrap,
        cli::{self, app, warn_for_deprecated_arguments, DefaultArgs},
        dashboard::Dashboard,
        ledger_lockfile, lock_ledger, new_spinner_progress_bar, println_name_value,
        redirect_stderr_to_file,
    },
    clap::{crate_name, value_t, value_t_or_exit, values_t, values_t_or_exit, ArgMatches},
    console::style,
    crossbeam_channel::unbounded,
    log::*,
    rand::{seq::SliceRandom, thread_rng},
    solana_accounts_db::{
        accounts_db::{AccountShrinkThreshold, AccountsDb, AccountsDbConfig, CreateAncientStorage},
        accounts_file::StorageAccess,
        accounts_index::{
            AccountIndex, AccountSecondaryIndexes, AccountSecondaryIndexesIncludeExclude,
            AccountsIndexConfig, IndexLimitMb, ScanFilter,
        },
        utils::{
            create_all_accounts_run_and_snapshot_dirs, create_and_canonicalize_directories,
            create_and_canonicalize_directory,
        },
    },
    solana_clap_utils::input_parsers::{keypair_of, keypairs_of, pubkey_of, value_of, values_of},
    solana_core::{
        banking_trace::DISABLED_BAKING_TRACE_DIR,
        consensus::tower_storage,
        system_monitor_service::SystemMonitorService,
        tpu::DEFAULT_TPU_COALESCE,
        validator::{
            is_snapshot_config_valid, BlockProductionMethod, BlockVerificationMethod, Validator,
            ValidatorConfig, ValidatorError, ValidatorStartProgress, ValidatorTpuConfig,
        },
    },
    solana_gossip::{
        cluster_info::{Node, NodeConfig},
        contact_info::ContactInfo,
    },
    solana_ledger::{
        blockstore_cleanup_service::{DEFAULT_MAX_LEDGER_SHREDS, DEFAULT_MIN_MAX_LEDGER_SHREDS},
        blockstore_options::{
            AccessType, BlockstoreCompressionType, BlockstoreOptions, BlockstoreRecoveryMode,
            LedgerColumnOptions,
        },
        use_snapshot_archives_at_startup::{self, UseSnapshotArchivesAtStartup},
    },
    solana_perf::recycler::enable_recycler_warming,
    solana_poh::poh_service,
    solana_rpc::{
        rpc::{JsonRpcConfig, RpcBigtableConfig},
        rpc_pubsub_service::PubSubConfig,
    },
    solana_rpc_client::rpc_client::RpcClient,
    solana_rpc_client_api::config::RpcLeaderScheduleConfig,
    solana_runtime::{
        runtime_config::RuntimeConfig,
        snapshot_bank_utils::DISABLED_SNAPSHOT_ARCHIVE_INTERVAL,
        snapshot_config::{SnapshotConfig, SnapshotUsage},
        snapshot_utils::{self, ArchiveFormat, SnapshotVersion},
    },
    solana_sdk::{
        clock::{Slot, DEFAULT_S_PER_SLOT},
        commitment_config::CommitmentConfig,
        hash::Hash,
        pubkey::Pubkey,
        signature::{read_keypair, Keypair, Signer},
    },
    solana_send_transaction_service::send_transaction_service,
    solana_streamer::socket::SocketAddrSpace,
    solana_tpu_client::tpu_client::DEFAULT_TPU_ENABLE_UDP,
    std::{
        collections::{HashSet, VecDeque},
        env,
        fs::{self, File},
        net::{IpAddr, Ipv4Addr, SocketAddr},
        num::NonZeroUsize,
        path::{Path, PathBuf},
        process::exit,
        str::FromStr,
        sync::{Arc, RwLock},
        time::{Duration, SystemTime},
    },
};

/// 替换内存分配器，Jemalloc多线程性能优于标准库使用的系统默认分配器
/// msvc 和 freebsd 不适用
#[cfg(not(any(target_env = "msvc", target_os = "freebsd")))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

/// 操作，只初始化环境或者实际运行
#[derive(Debug, PartialEq, Eq)]
enum Operation {
    /// 只初始化环境
    Initialize,
    /// 实际运行
    Run,
}

/// 每秒的毫秒数
const MILLIS_PER_SECOND: u64 = 1000;

/// 用来在控制台上打印启动进度条、启动后定时打印验证器状态
fn monitor_validator(ledger_path: &Path) {
    // 新建监控仪表板
    let dashboard = Dashboard::new(ledger_path, None, None).unwrap_or_else(|err| {
        println!(
            "Error: Unable to connect to validator at {}: {:?}",
            ledger_path.display(),
            err,
        );
        exit(1);
    });
    // 运行监控仪表板
    dashboard.run(Duration::from_secs(2));
}

/// 等待重启窗口，找合适的时机以及自己状态健康的时候，适合重启，算重启窗口
/// 函数会判断是否满足以下条件之一，进入重启窗口：
/// 节点健康且没有高过失的质押。
/// 有合适的空闲时间段（大于最小空闲时间的 slot）。
/// 如果跳过了快照检查且其他条件满足，则直接进入重启。
/// 如果快照状态合适（比如有增量快照），并且其他条件满足，则进入重启
fn wait_for_restart_window(
    ledger_path: &Path,
    identity: Option<Pubkey>,
    min_idle_time_in_minutes: usize,
    max_delinquency_percentage: u8,
    skip_new_snapshot_check: bool,
    skip_health_check: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    /// 设置每次循环检查的间隔时间为 5 秒。
    let sleep_interval = Duration::from_secs(5);

    /// 每分钟最小空闲时隙，将最小空闲时间转换为最小空闲 slot 数量。
    /// DEFAULT_S_PER_SLOT 是每个 slot 所需的时间（默认为 400 毫秒）
    /// 该值会用于计算验证者节点在重启前至少需要有多少个 slot 的空闲时间
    /// 需要挑一个长度足够长的空闲时隙，这段时间自己离任期还有很远，可以从容在这段时间内重启完成不会手忙脚乱
    /// 这里设置的就是这个空闲时隙的最小值
    let min_idle_slots = (min_idle_time_in_minutes as f64 * 60. / DEFAULT_S_PER_SLOT) as Slot;

    /// 新建 admin 客户端，通过本地 sockets 文件连接
    let admin_client = admin_rpc_service::connect(ledger_path);
    /// 获取验证器正常 rpc 地址
    let rpc_addr = admin_rpc_service::runtime()
        .block_on(async move { admin_client.await?.rpc_addr().await })
        .map_err(|err| format!("Unable to get validator RPC address: {err}"))?;

    /// 新建 rpc
    let Some(rpc_client) = rpc_addr.map(RpcClient::new_socket) else {
        return Err("RPC not available".into());
    };

    /// 获取并打印节点公钥 从 rpc
    let my_identity = rpc_client.get_identity()?;
    let identity = identity.unwrap_or(my_identity);
    let monitoring_another_validator = identity != my_identity;
    println_name_value("Identity:", &identity.to_string());
    println_name_value(
        "Minimum Idle Time:",
        &format!("{min_idle_slots} slots (~{min_idle_time_in_minutes} minutes)"),
    );

    println!("Maximum permitted delinquency: {max_delinquency_percentage}%");

    let mut current_epoch = None;
    let mut leader_schedule = VecDeque::new();
    let mut restart_snapshot = None;
    let mut upcoming_idle_windows = vec![]; // Vec<(starting slot, idle window length in slots)>

    let progress_bar = new_spinner_progress_bar();
    let monitor_start_time = SystemTime::now();

    let mut seen_incremential_snapshot = false;
    loop {
        /// 获取验证器上有快照的最高时隙，返回一个 full 快照，和一个可能的增量快照
        let snapshot_slot_info = rpc_client.get_highest_snapshot_slot().ok();
        /// 最高时隙有没有增量
        let snapshot_slot_info_has_incremential = snapshot_slot_info
            .as_ref()
            .map(|snapshot_slot_info| snapshot_slot_info.incremental.is_some())
            .unwrap_or_default();
        seen_incremential_snapshot |= snapshot_slot_info_has_incremential;

        /// 获取纪元信息
        let epoch_info = rpc_client.get_epoch_info_with_commitment(CommitmentConfig::processed())?;
        /// 健康检查
        let healthy = skip_health_check || rpc_client.get_health().ok().is_some();
        /// 过失质押和总质押的比值
        /// 如果过失的质押比例超过了最大拖延百分比（max_delinquency_percentage），则不允许重启。
        let delinquent_stake_percentage = {
            let vote_accounts = rpc_client.get_vote_accounts()?;
            let current_stake: u64 = vote_accounts
                .current
                .iter()
                .map(|va| va.activated_stake)
                .sum();
            let delinquent_stake: u64 = vote_accounts
                .delinquent
                .iter()
                .map(|va| va.activated_stake)
                .sum();
            let total_stake = current_stake + delinquent_stake;
            delinquent_stake as f64 / total_stake as f64
        };

        /// 每个纪元只计算一次
        if match current_epoch {
            None => true,
            Some(current_epoch) => current_epoch != epoch_info.epoch,
        } {
            /// 打印进度，获取 leader 排期
            progress_bar.set_message(format!(
                "Fetching leader schedule for epoch {}...",
                epoch_info.epoch
            ));
            /// 获取纪元内第一个时隙，用当前时隙减去当前时隙在纪元内索引，因为时隙是递增的数字
            let first_slot_in_epoch = epoch_info.absolute_slot - epoch_info.slot_index;
            leader_schedule = rpc_client
                ///获取 leader 排期，是一个 节点公钥 到 纪元内时隙序号 的映射表
                .get_leader_schedule_with_config(
                    Some(first_slot_in_epoch),
                    RpcLeaderScheduleConfig {
                        identity: Some(identity.to_string()),
                        ..RpcLeaderScheduleConfig::default()
                    },
                )?
                .ok_or_else(|| {
                    format!("Unable to get leader schedule from slot {first_slot_in_epoch}")
                })?
                /// 取自己的 leader 任期
                .get(&identity.to_string())
                .cloned()
                .unwrap_or_default()
                .into_iter()
                /// 从纪元内时隙序号转到绝对的时隙号
                .map(|slot_index| first_slot_in_epoch.saturating_add(slot_index as u64))
                .filter(|slot| *slot > epoch_info.absolute_slot)
                .collect::<VecDeque<_>>();

            /// 可以用来启动的非任期窗口
            upcoming_idle_windows.clear();
            {
                let mut leader_schedule = leader_schedule.clone();
                /// 最大的非任期窗口
                let mut max_idle_window = 0;

                /// 窗口开始时隙设为当前时隙号
                let mut idle_window_start_slot = epoch_info.absolute_slot;
                /// 遍历自己的任期
                while let Some(next_leader_slot) = leader_schedule.pop_front() {
                    /// 下一次自己的任期时隙减去当前时隙号，即为非任期窗口
                    let idle_window = next_leader_slot - idle_window_start_slot;
                    /// 更新最大非任期窗口
                    max_idle_window = max_idle_window.max(idle_window);
                    /// 如果长度超过了设置的非任期窗口最小时隙，那就算是可以用来启动的非任期窗口，记录到可以用来启动的非任期窗口里
                    if idle_window > min_idle_slots {
                        upcoming_idle_windows.push((idle_window_start_slot, idle_window));
                    }
                    /// 下一个非任期窗口从下一届任期开始
                    idle_window_start_slot = next_leader_slot;
                }
                /// 如果没有任期或可以用来启动的非任期窗口，则报错
                if !leader_schedule.is_empty() && upcoming_idle_windows.is_empty() {
                    return Err(format!(
                        "Validator has no idle window of at least {} slots. Largest idle window \
                         for epoch {} is {} slots",
                        min_idle_slots, epoch_info.epoch, max_idle_window
                    )
                    .into());
                }
            }

            /// 更新纪元
            current_epoch = Some(epoch_info.epoch);
        }

        let status = {
            /// 不健康就不继续查状态了
            if !healthy {
                style("Node is unhealthy").red().to_string()
            } else {
                // Wait until a hole in the leader schedule before restarting the node
                // 获取现在是否是一个启动好时机
                let in_leader_schedule_hole = if epoch_info.slot_index + min_idle_slots
                    > epoch_info.slots_in_epoch
                {
                    /// 如果纪元里剩余的时隙不够最小非任期窗口，那就等下一个纪元
                    Err("Current epoch is almost complete".to_string())
                } else {
                    /// 排除掉已经过时的时隙
                    while leader_schedule
                        .front()
                        .map(|slot| *slot < epoch_info.absolute_slot)
                        .unwrap_or(false)
                    {
                        leader_schedule.pop_front();
                    }
                    /// 排除掉已经过时的非任期窗口
                    while upcoming_idle_windows
                        .first()
                        .map(|(slot, _)| *slot < epoch_info.absolute_slot)
                        .unwrap_or(false)
                    {
                        upcoming_idle_windows.pop();
                    }

                    /// 再看调度表
                    match leader_schedule.front() {
                        None => {
                            /// 已经没有任期了，可以安全启动
                            Ok(()) // Validator has no leader slots
                        }
                        Some(next_leader_slot) => {
                            /// 下一次任期开始时隙减去当前时隙
                            let idle_slots =
                                next_leader_slot.saturating_sub(epoch_info.absolute_slot);
                            if idle_slots >= min_idle_slots {
                                /// 下一次任期还早，可以安全启动
                                Ok(())
                            } else {
                                /// 下一次任期不早了，赶不上启动
                                Err(match upcoming_idle_windows.first() {
                                    Some((starting_slot, length_in_slots)) => {
                                        format!(
                                            "Next idle window in {} slots, for {} slots",
                                            starting_slot.saturating_sub(epoch_info.absolute_slot),
                                            length_in_slots
                                        )
                                    }
                                    None => format!(
                                        "Validator will be leader soon. Next leader slot is \
                                         {next_leader_slot}"
                                    ),
                                })
                            }
                        }
                    }
                };

                /// 现在是否是个启动好时机
                match in_leader_schedule_hole {
                    Ok(_) => {
                        /// 下面都是需要快照的检查了，不检查可以直接启动
                        if skip_new_snapshot_check {
                            break; // Restart!
                        }
                        let snapshot_slot = snapshot_slot_info.map(|snapshot_slot_info| {
                            snapshot_slot_info
                                .incremental
                                .unwrap_or(snapshot_slot_info.full)
                        });
                        if restart_snapshot.is_none() {
                            restart_snapshot = snapshot_slot;
                        }
                        /// 没有新快照
                        if restart_snapshot == snapshot_slot && !monitoring_another_validator {
                            "Waiting for a new snapshot".to_string()
                        /// 过失率过高
                        } else if delinquent_stake_percentage
                            >= (max_delinquency_percentage as f64 / 100.)
                        {
                            style("Delinquency too high").red().to_string()
                        } else if seen_incremential_snapshot && !snapshot_slot_info_has_incremential
                        {
                            // Restarts using just a full snapshot will put the node significantly
                            // further behind than if an incremental snapshot is also used, as full
                            // snapshots are larger and take much longer to create.
                            //
                            // Therefore if the node just created a new full snapshot, wait a
                            // little longer until it creates the first incremental snapshot for
                            // the full snapshot.
                            "Waiting for incremental snapshot".to_string()
                        } else {
                            break; // Restart!
                        }
                    }
                    /// 不是个好时机
                    Err(why) => style(why).yellow().to_string(),
                }
            }
        };

        /// 如果可以重启，在上面那个 break 那里就已经退出函数了，这里打印进度条，说明还不能重启
        /// monitor 开始到现在经过的时间 | 当前时隙（也是 Processed 时隙） | 过失比例 | 不启动的原因
        progress_bar.set_message(format!(
            "{} | Processed Slot: {} {} | {:.2}% delinquent stake | {}",
            {
                let elapsed =
                    chrono::Duration::from_std(monitor_start_time.elapsed().unwrap()).unwrap();

                format!(
                    "{:02}:{:02}:{:02}",
                    elapsed.num_hours(),
                    elapsed.num_minutes() % 60,
                    elapsed.num_seconds() % 60
                )
            },
            epoch_info.absolute_slot,
            if monitoring_another_validator {
                "".to_string()
            } else {
                format!(
                    "| Full Snapshot Slot: {} | Incremental Snapshot Slot: {}",
                    snapshot_slot_info
                        .as_ref()
                        .map(|snapshot_slot_info| snapshot_slot_info.full.to_string())
                        .unwrap_or_else(|| '-'.to_string()),
                    snapshot_slot_info
                        .as_ref()
                        .and_then(|snapshot_slot_info| snapshot_slot_info
                            .incremental
                            .map(|incremental| incremental.to_string()))
                        .unwrap_or_else(|| '-'.to_string()),
                )
            },
            delinquent_stake_percentage * 100.,
            status
        ));
        std::thread::sleep(sleep_interval);
    }
    /// 关闭进度条显示
    drop(progress_bar);
    /// 已做好重启准备
    println!("{}", style("Ready to restart").green());
    Ok(())
}

/// 通过本地 admin 客户端，请求更新 repair 白名单
fn set_repair_whitelist(
    ledger_path: &Path,
    whitelist: Vec<Pubkey>,
) -> Result<(), Box<dyn std::error::Error>> {
    let admin_client = admin_rpc_service::connect(ledger_path);
    admin_rpc_service::runtime()
        .block_on(async move { admin_client.await?.set_repair_whitelist(whitelist).await })
        .map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("setRepairWhitelist request failed: {err}"),
            )
        })?;
    Ok(())
}

// This function is duplicated in ledger-tool/src/main.rs...
/// 解析硬分叉传参，必须是个u64
fn hardforks_of(matches: &ArgMatches<'_>, name: &str) -> Option<Vec<Slot>> {
    if matches.is_present(name) {
        Some(values_t_or_exit!(matches, name, Slot))
    } else {
        None
    }
}

/// 解析验证器集传参
/// 或许这里newtype更好？
fn validators_set(
    identity_pubkey: &Pubkey,
    matches: &ArgMatches<'_>,
    matches_name: &str,
    arg_name: &str,
) -> Option<HashSet<Pubkey>> {
    if matches.is_present(matches_name) {
        let validators_set: HashSet<_> = values_t_or_exit!(matches, matches_name, Pubkey)
            .into_iter()
            .collect();
        if validators_set.contains(identity_pubkey) {
            eprintln!("The validator's identity pubkey cannot be a {arg_name}: {identity_pubkey}");
            exit(1);
        }
        Some(validators_set)
    } else {
        None
    }
}

/// 获取集群shred版本，用于后续通信
/// 在 Solana 区块链中，**Shred Version** 是一个与网络分片相关的重要概念，用于确保不同的集群（Cluster）之间的数据兼容性和网络隔离性。以下是详细解释：
/// ---
/// ### 1. **Shred 和 Shred Version 的背景**
/// - **Shred** 是 Solana 用于区块链数据存储和传播的基本数据单位。
///   - 当一个区块生成时，区块被分割成多个小数据块，这些小块称为 Shreds。
///   - Shreds 被用于高效传输和存储数据，同时支持验证者在网络中的数据同步。
/// - **Shred Version** 是一个网络中的唯一标识符。
///   - 它通过一个特定的计算公式生成，通常基于当前集群的区块链状态（例如 Genesis Hash）。
///   - 每个集群有一个独立的 Shred Version，用来确保只接受来自相同 Shred Version 的数据。
/// ---
/// ### 2. **Shred Version 的作用**
/// 1. **隔离网络数据**
///    - 不同的网络或集群（如主网、测试网、开发网）会有不同的 Shred Version。
///    - 这可以防止网络之间的数据混淆或干扰。例如，主网不会意外地处理测试网的 Shred 数据。
/// 2. **防止冲突和安全性**
///    - 如果两条链的 Shred Version 相同，可能会导致冲突或意外行为。
///    - 唯一的 Shred Version 可防止意外的数据传输和攻击。
/// 3. **集群验证**
///    - 节点在加入一个网络时，会检查 Shred Version。如果本地的 Shred Version 与集群的不同，节点将无法同步数据。
/// ---
/// ### 3. **Shred Version 的生成**
/// - Shred Version 通常通过以下公式计算：
///   ```
///   Shred Version = hash(Genesis Hash) % MAX_VERSION
///   ```
///   - **Genesis Hash** 是链启动时的初始哈希值，用于唯一标识区块链。
///   - **MAX_VERSION** 是一个预定义的常量，限制 Shred Version 的范围。
/// ---
/// ### 4. **与 Validator 的关系**
/// - **Validator（验证者）** 在启动时必须配置正确的 Shred Version。
/// - 如果 Shred Version 不匹配：
///   - 节点无法加入集群。
///   - 无法处理网络中的交易或数据。
/// ---
/// ### 5. **实际场景举例**
/// - **主网升级**：当 Solana 主网进行重大升级时，可能会更新 Genesis Hash，从而改变 Shred Version。这要求所有节点更新其配置以匹配新的版本。
/// - **测试网和开发环境**：开发者在本地运行 Solana 节点时，会生成一个新的 Genesis Hash，因此也会有不同的 Shred Version。
/// ---
/// ### 总结
/// Shred Version 是 Solana 中用于分片版本管理的重要机制，通过唯一标识不同集群的数据流，确保了数据隔离性和网络安全性。
fn get_cluster_shred_version(entrypoints: &[SocketAddr]) -> Option<u16> {
    let entrypoints = {
        let mut index: Vec<_> = (0..entrypoints.len()).collect();
        index.shuffle(&mut rand::thread_rng());
        index.into_iter().map(|i| &entrypoints[i])
    };
    /// 从 entrypoints 获取
    for entrypoint in entrypoints {
        match solana_net_utils::get_cluster_shred_version(entrypoint) {
            Err(err) => eprintln!("get_cluster_shred_version failed: {entrypoint}, {err}"),
            Ok(0) => eprintln!("entrypoint {entrypoint} returned shred-version zero"),
            Ok(shred_version) => {
                info!(
                    "obtained shred-version {} from {}",
                    shred_version, entrypoint
                );
                return Some(shred_version);
            }
        }
    }
    None
}

/// 解析并配置banking trace的目录大小限制传参
fn configure_banking_trace_dir_byte_limit(
    validator_config: &mut ValidatorConfig,
    matches: &ArgMatches,
) {
    validator_config.banking_trace_dir_byte_limit = if matches.is_present("disable_banking_trace") {
        // disable with an explicit flag; This effectively becomes `opt-out` by resetting to
        // DISABLED_BAKING_TRACE_DIR, while allowing us to specify a default sensible limit in clap
        // configuration for cli help.
        DISABLED_BAKING_TRACE_DIR
    } else {
        // a default value in clap configuration (BANKING_TRACE_DIR_DEFAULT_BYTE_LIMIT) or
        // explicit user-supplied override value
        value_t_or_exit!(matches, "banking_trace_dir_byte_limit", u64)
    };
}

pub fn main() {
    /// 默认配置，用来在命令行提示中显示默认值，并且作为实际的默认值
    let default_args = DefaultArgs::new();
    /// 获取 solana 版本，crate 的版本，仓库的哈希
    let solana_version = solana_version::version!();
    /// 解析命令行参数
    let cli_app = app(solana_version, &default_args);
    let matches = cli_app.get_matches();
    /// 警告弃用的参数
    warn_for_deprecated_arguments(&matches);

    /// ip范围，只监听公网，还是内网公网都可以
    let socket_addr_space = SocketAddrSpace::new(matches.is_present("allow_private_addr"));
    /// 账本路径
    let ledger_path = PathBuf::from(matches.value_of("ledger_path").unwrap());

    /// 子命令，除了运行验证器主程序之外的命令，都直接在匹配支上解决
    let operation = match matches.subcommand() {
        /// 默认是运行验证器主程序
        ("", _) | ("run", _) => Operation::Run,
        /// 管理 投票者账户
        ("authorized-voter", Some(authorized_voter_subcommand_matches)) => {
            match authorized_voter_subcommand_matches.subcommand() {
                /// 添加 投票者账户
                ("add", Some(subcommand_matches)) => {
                    /// 取密钥对路径
                    if let Ok(authorized_voter_keypair) =
                        value_t!(subcommand_matches, "authorized_voter_keypair", String)
                    {
                        /// canonicalize 在路径不存在时就会报错
                        let authorized_voter_keypair = fs::canonicalize(&authorized_voter_keypair)
                            .unwrap_or_else(|err| {
                                println!(
                                    "Unable to access path: {authorized_voter_keypair}: {err:?}"
                                );
                                exit(1);
                            });
                        println!(
                            "Adding authorized voter path: {}",
                            authorized_voter_keypair.display()
                        );

                        /// 要用本地 rpc 来添加
                        let admin_client = admin_rpc_service::connect(&ledger_path);
                        /// tokio 阻塞线程
                        admin_rpc_service::runtime()
                            .block_on(async move {
                                admin_client
                                    .await?
                                    /// 添加投票者的 rpc
                                    .add_authorized_voter(
                                        authorized_voter_keypair.display().to_string(),
                                    )
                                    .await
                            })
                            .unwrap_or_else(|err| {
                                println!("addAuthorizedVoter request failed: {err}");
                                exit(1);
                            });
                    } else {
                        /// 没从参数指定密钥对的话，就从标准输入取
                        let mut stdin = std::io::stdin();
                        let authorized_voter_keypair =
                            read_keypair(&mut stdin).unwrap_or_else(|err| {
                                println!("Unable to read JSON keypair from stdin: {err:?}");
                                exit(1);
                            });
                        println!(
                            "Adding authorized voter: {}",
                            authorized_voter_keypair.pubkey()
                        );

                        /// 要用本地 rpc 来添加
                        let admin_client = admin_rpc_service::connect(&ledger_path);
                        admin_rpc_service::runtime()
                            .block_on(async move {
                                admin_client
                                    .await?
                                    .add_authorized_voter_from_bytes(Vec::from(
                                        authorized_voter_keypair.to_bytes(),
                                    ))
                                    .await
                            })
                            .unwrap_or_else(|err| {
                                println!("addAuthorizedVoterFromBytes request failed: {err}");
                                exit(1);
                            });
                    }

                    return;
                }
                /// 移除所有投票者
                ("remove-all", _) => {
                    /// 靠发送本地 admin rpc 请求，走本地文件socket，所以很安全
                    let admin_client = admin_rpc_service::connect(&ledger_path);
                    admin_rpc_service::runtime()
                        .block_on(async move {
                            admin_client.await?.remove_all_authorized_voters().await
                        })
                        .unwrap_or_else(|err| {
                            println!("removeAllAuthorizedVoters request failed: {err}");
                            exit(1);
                        });
                    println!("All authorized voters removed");
                    return;
                }
                _ => unreachable!(),
            }
        }
        /// 管理 geyser 插件，依然是靠本地 rpc
        ("plugin", Some(plugin_subcommand_matches)) => {
            match plugin_subcommand_matches.subcommand() {
                /// 列出插件
                ("list", _) => {
                    let admin_client = admin_rpc_service::connect(&ledger_path);
                    let plugins = admin_rpc_service::runtime()
                        .block_on(async move { admin_client.await?.list_plugins().await })
                        .unwrap_or_else(|err| {
                            println!("Failed to list plugins: {err}");
                            exit(1);
                        });
                    if !plugins.is_empty() {
                        println!("Currently the following plugins are loaded:");
                        for (plugin, i) in plugins.into_iter().zip(1..) {
                            println!("  {i}) {plugin}");
                        }
                    } else {
                        println!("There are currently no plugins loaded");
                    }
                    return;
                }
                /// 上传插件
                ("unload", Some(subcommand_matches)) => {
                    if let Ok(name) = value_t!(subcommand_matches, "name", String) {
                        let admin_client = admin_rpc_service::connect(&ledger_path);
                        admin_rpc_service::runtime()
                            .block_on(async {
                                admin_client.await?.unload_plugin(name.clone()).await
                            })
                            .unwrap_or_else(|err| {
                                println!("Failed to unload plugin {name}: {err:?}");
                                exit(1);
                            });
                        println!("Successfully unloaded plugin: {name}");
                    }
                    return;
                }
                /// 加载插件
                ("load", Some(subcommand_matches)) => {
                    if let Ok(config) = value_t!(subcommand_matches, "config", String) {
                        let admin_client = admin_rpc_service::connect(&ledger_path);
                        let name = admin_rpc_service::runtime()
                            .block_on(async {
                                admin_client.await?.load_plugin(config.clone()).await
                            })
                            .unwrap_or_else(|err| {
                                println!("Failed to load plugin {config}: {err:?}");
                                exit(1);
                            });
                        println!("Successfully loaded plugin: {name}");
                    }
                    return;
                }
                /// 重新加载插件
                ("reload", Some(subcommand_matches)) => {
                    if let Ok(name) = value_t!(subcommand_matches, "name", String) {
                        if let Ok(config) = value_t!(subcommand_matches, "config", String) {
                            let admin_client = admin_rpc_service::connect(&ledger_path);
                            admin_rpc_service::runtime()
                                .block_on(async {
                                    admin_client
                                        .await?
                                        .reload_plugin(name.clone(), config.clone())
                                        .await
                                })
                                .unwrap_or_else(|err| {
                                    println!("Failed to reload plugin {name}: {err:?}");
                                    exit(1);
                                });
                            println!("Successfully reloaded plugin: {name}");
                        }
                    }
                    return;
                }
                _ => unreachable!(),
            }
        }
        /// 打印通信信息
        ("contact-info", Some(subcommand_matches)) => {
            let output_mode = subcommand_matches.value_of("output");
            let admin_client = admin_rpc_service::connect(&ledger_path);
            let contact_info = admin_rpc_service::runtime()
                .block_on(async move { admin_client.await?.contact_info().await })
                .unwrap_or_else(|err| {
                    eprintln!("Contact info query failed: {err}");
                    exit(1);
                });
            if let Some(mode) = output_mode {
                match mode {
                    "json" => println!("{}", serde_json::to_string_pretty(&contact_info).unwrap()),
                    "json-compact" => print!("{}", serde_json::to_string(&contact_info).unwrap()),
                    _ => unreachable!(),
                }
            } else {
                print!("{contact_info}");
            }
            return;
        }
        /// 不实际启动，但是做启动工作，和启动后立刻停掉差不多
        ("init", _) => Operation::Initialize,
        /// 退出正在运行的验证器
        ("exit", Some(subcommand_matches)) => {
            let min_idle_time = value_t_or_exit!(subcommand_matches, "min_idle_time", usize);
            let force = subcommand_matches.is_present("force");
            let monitor = subcommand_matches.is_present("monitor");
            let skip_new_snapshot_check = subcommand_matches.is_present("skip_new_snapshot_check");
            let skip_health_check = subcommand_matches.is_present("skip_health_check");
            let max_delinquent_stake =
                value_t_or_exit!(subcommand_matches, "max_delinquent_stake", u8);

            /// 不是强制的就先等合适的时机，因为这个函数，所以也要传那些跳过检查的选项进来
            if !force {
                wait_for_restart_window(
                    &ledger_path,
                    None,
                    min_idle_time,
                    max_delinquent_stake,
                    skip_new_snapshot_check,
                    skip_health_check,
                )
                .unwrap_or_else(|err| {
                    println!("{err}");
                    exit(1);
                });
            }

            /// 依然是用 rpc
            let admin_client = admin_rpc_service::connect(&ledger_path);
            admin_rpc_service::runtime()
                .block_on(async move { admin_client.await?.exit().await })
                .unwrap_or_else(|err| {
                    println!("exit request failed: {err}");
                    exit(1);
                });
            println!("Exit request sent");

            /// 监控关闭状态
            if monitor {
                monitor_validator(&ledger_path);
            }
            return;
        }
        /// 监控验证器
        ("monitor", _) => {
            monitor_validator(&ledger_path);
            return;
        }
        /// 覆盖质押节点，本地 rpc
        ("staked-nodes-overrides", Some(subcommand_matches)) => {
            if !subcommand_matches.is_present("path") {
                println!(
                    "staked-nodes-overrides requires argument of location of the configuration"
                );
                exit(1);
            }

            let path = subcommand_matches.value_of("path").unwrap();

            let admin_client = admin_rpc_service::connect(&ledger_path);
            admin_rpc_service::runtime()
                .block_on(async move {
                    admin_client
                        .await?
                        .set_staked_nodes_overrides(path.to_string())
                        .await
                })
                .unwrap_or_else(|err| {
                    println!("setStakedNodesOverrides request failed: {err}");
                    exit(1);
                });
            return;
        }
        /// 重设节点身份，本地 rpc
        ("set-identity", Some(subcommand_matches)) => {
            let require_tower = subcommand_matches.is_present("require_tower");

            if let Ok(identity_keypair) = value_t!(subcommand_matches, "identity", String) {
                let identity_keypair = fs::canonicalize(&identity_keypair).unwrap_or_else(|err| {
                    println!("Unable to access path: {identity_keypair}: {err:?}");
                    exit(1);
                });
                println!(
                    "New validator identity path: {}",
                    identity_keypair.display()
                );

                let admin_client = admin_rpc_service::connect(&ledger_path);
                admin_rpc_service::runtime()
                    .block_on(async move {
                        admin_client
                            .await?
                            .set_identity(identity_keypair.display().to_string(), require_tower)
                            .await
                    })
                    .unwrap_or_else(|err| {
                        println!("setIdentity request failed: {err}");
                        exit(1);
                    });
            } else {
                let mut stdin = std::io::stdin();
                let identity_keypair = read_keypair(&mut stdin).unwrap_or_else(|err| {
                    println!("Unable to read JSON keypair from stdin: {err:?}");
                    exit(1);
                });
                println!("New validator identity: {}", identity_keypair.pubkey());

                let admin_client = admin_rpc_service::connect(&ledger_path);
                admin_rpc_service::runtime()
                    .block_on(async move {
                        admin_client
                            .await?
                            .set_identity_from_bytes(
                                Vec::from(identity_keypair.to_bytes()),
                                require_tower,
                            )
                            .await
                    })
                    .unwrap_or_else(|err| {
                        println!("setIdentityFromBytes request failed: {err}");
                        exit(1);
                    });
            };

            return;
        }
        /// 设置 log 过滤器，本地 rpc
        ("set-log-filter", Some(subcommand_matches)) => {
            let filter = value_t_or_exit!(subcommand_matches, "filter", String);
            let admin_client = admin_rpc_service::connect(&ledger_path);
            admin_rpc_service::runtime()
                .block_on(async move { admin_client.await?.set_log_filter(filter).await })
                .unwrap_or_else(|err| {
                    println!("set log filter failed: {err}");
                    exit(1);
                });
            return;
        }
        /// 看看是否适合重启
        ("wait-for-restart-window", Some(subcommand_matches)) => {
            let min_idle_time = value_t_or_exit!(subcommand_matches, "min_idle_time", usize);
            let identity = pubkey_of(subcommand_matches, "identity");
            let max_delinquent_stake =
                value_t_or_exit!(subcommand_matches, "max_delinquent_stake", u8);
            let skip_new_snapshot_check = subcommand_matches.is_present("skip_new_snapshot_check");
            let skip_health_check = subcommand_matches.is_present("skip_health_check");

            wait_for_restart_window(
                &ledger_path,
                identity,
                min_idle_time,
                max_delinquent_stake,
                skip_new_snapshot_check,
                skip_health_check,
            )
            .unwrap_or_else(|err| {
                println!("{err}");
                exit(1);
            });
            return;
        }
        /// 从别的节点拉取数据，修复碎屑，本地 rpc
        ("repair-shred-from-peer", Some(subcommand_matches)) => {
            let pubkey = value_t!(subcommand_matches, "pubkey", Pubkey).ok();
            let slot = value_t_or_exit!(subcommand_matches, "slot", u64);
            let shred_index = value_t_or_exit!(subcommand_matches, "shred", u64);
            let admin_client = admin_rpc_service::connect(&ledger_path);
            admin_rpc_service::runtime()
                .block_on(async move {
                    admin_client
                        .await?
                        .repair_shred_from_peer(pubkey, slot, shred_index)
                        .await
                })
                .unwrap_or_else(|err| {
                    println!("repair shred from peer failed: {err}");
                    exit(1);
                });
            return;
        }
        /// 管理白名单
        ("repair-whitelist", Some(repair_whitelist_subcommand_matches)) => {
            match repair_whitelist_subcommand_matches.subcommand() {
                /// 获取
                ("get", Some(subcommand_matches)) => {
                    let output_mode = subcommand_matches.value_of("output");
                    let admin_client = admin_rpc_service::connect(&ledger_path);
                    let repair_whitelist = admin_rpc_service::runtime()
                        .block_on(async move { admin_client.await?.repair_whitelist().await })
                        .unwrap_or_else(|err| {
                            eprintln!("Repair whitelist query failed: {err}");
                            exit(1);
                        });
                    if let Some(mode) = output_mode {
                        match mode {
                            "json" => println!(
                                "{}",
                                serde_json::to_string_pretty(&repair_whitelist).unwrap()
                            ),
                            "json-compact" => {
                                print!("{}", serde_json::to_string(&repair_whitelist).unwrap())
                            }
                            _ => unreachable!(),
                        }
                    } else {
                        print!("{repair_whitelist}");
                    }
                    return;
                }
                /// 设置
                ("set", Some(subcommand_matches)) => {
                    let whitelist = if subcommand_matches.is_present("whitelist") {
                        let validators_set: HashSet<_> =
                            values_t_or_exit!(subcommand_matches, "whitelist", Pubkey)
                                .into_iter()
                                .collect();
                        validators_set.into_iter().collect::<Vec<_>>()
                    } else {
                        return;
                    };
                    set_repair_whitelist(&ledger_path, whitelist).unwrap_or_else(|err| {
                        eprintln!("{err}");
                        exit(1);
                    });
                    return;
                }
                /// 删除全部
                ("remove-all", _) => {
                    set_repair_whitelist(&ledger_path, Vec::default()).unwrap_or_else(|err| {
                        eprintln!("{err}");
                        exit(1);
                    });
                    return;
                }
                _ => unreachable!(),
            }
        }
        /// 重设公网监听地址，本地 rpc
        ("set-public-address", Some(subcommand_matches)) => {
            let parse_arg_addr = |arg_name: &str, arg_long: &str| -> Option<SocketAddr> {
                subcommand_matches.value_of(arg_name).map(|host_port| {
                    solana_net_utils::parse_host_port(host_port).unwrap_or_else(|err| {
                        eprintln!(
                            "Failed to parse --{arg_long} address. It must be in the HOST:PORT \
                             format. {err}"
                        );
                        exit(1);
                    })
                })
            };
            let tpu_addr = parse_arg_addr("tpu_addr", "tpu");
            let tpu_forwards_addr = parse_arg_addr("tpu_forwards_addr", "tpu-forwards");

            macro_rules! set_public_address {
                ($public_addr:expr, $set_public_address:ident, $request:literal) => {
                    if let Some(public_addr) = $public_addr {
                        let admin_client = admin_rpc_service::connect(&ledger_path);
                        admin_rpc_service::runtime()
                            .block_on(async move {
                                admin_client.await?.$set_public_address(public_addr).await
                            })
                            .unwrap_or_else(|err| {
                                eprintln!("{} request failed: {err}", $request);
                                exit(1);
                            });
                    }
                };
            }
            set_public_address!(tpu_addr, set_public_tpu_address, "setPublicTpuAddress");
            set_public_address!(
                tpu_forwards_addr,
                set_public_tpu_forwards_address,
                "setPublicTpuForwardsAddress"
            );
            return;
        }
        _ => unreachable!(),
    };

    /// 解构线程数设置
    let cli::thread_args::NumThreadConfig {
        accounts_db_clean_threads,
        accounts_db_foreground_threads,
        accounts_db_hash_threads,
        accounts_index_flush_threads,
        ip_echo_server_threads,
        rayon_global_threads,
        replay_forks_threads,
        replay_transactions_threads,
        rocksdb_compaction_threads,
        rocksdb_flush_threads,
        tvu_receive_threads,
        tvu_sigverify_threads,
    } = cli::thread_args::parse_num_threads_args(&matches);

    /// 取验证器公钥
    let identity_keypair = keypair_of(&matches, "identity").unwrap_or_else(|| {
        clap::Error::with_description(
            "The --identity <KEYPAIR> argument is required",
            clap::ErrorKind::ArgumentNotFound,
        )
        .exit();
    });

    /// 取日志路径
    let logfile = {
        let logfile = matches
            .value_of("logfile")
            .map(|s| s.into())
            .unwrap_or_else(|| format!("agave-validator-{}.log", identity_keypair.pubkey()));

        if logfile == "-" {
            None
        } else {
            println!("log file: {logfile}");
            Some(logfile)
        }
    };
    let use_progress_bar = logfile.is_none();
    /// 重定向到文件
    let _logger_thread = redirect_stderr_to_file(logfile);

    /// 打印版本，回显启动参数
    info!("{} {}", crate_name!(), solana_version);
    info!("Starting validator with: {:#?}", std::env::args_os());

    /// 初始化 cuda
    let cuda = matches.is_present("cuda");
    if cuda {
        solana_perf::perf_libs::init_cuda();
        enable_recycler_warming();
    }

    /// 打印 cuda 和 avx 情况
    solana_core::validator::report_target_features();

    
    /// 投票者账户
    let authorized_voter_keypairs = keypairs_of(&matches, "authorized_voter_keypairs")
        .map(|keypairs| keypairs.into_iter().map(Arc::new).collect())
        .unwrap_or_else(|| {
            vec![Arc::new(
                keypair_of(&matches, "identity").expect("identity"),
            )]
        });
    let authorized_voter_keypairs = Arc::new(RwLock::new(authorized_voter_keypairs));

    /// 质押账户覆盖，得到一个 公钥 到 新质押值 的映射表
    let staked_nodes_overrides_path = matches
        .value_of("staked_nodes_overrides")
        .map(str::to_string);
    let staked_nodes_overrides = Arc::new(RwLock::new(
        match &staked_nodes_overrides_path {
            None => StakedNodesOverrides::default(),
            Some(p) => load_staked_nodes_overrides(p).unwrap_or_else(|err| {
                error!("Failed to load stake-nodes-overrides from {}: {}", p, err);
                clap::Error::with_description(
                    "Failed to load configuration of stake-nodes-overrides argument",
                    clap::ErrorKind::InvalidValue,
                )
                .exit()
            }),
        }
        .staked_map_id,
    ));

    /// 初始化完成文件
    let init_complete_file = matches.value_of("init_complete_file");

    /// rpc 相关的配置
    let rpc_bootstrap_config = bootstrap::RpcBootstrapConfig {
        no_genesis_fetch: matches.is_present("no_genesis_fetch"),
        no_snapshot_fetch: matches.is_present("no_snapshot_fetch"),
        check_vote_account: matches
            .value_of("check_vote_account")
            .map(|url| url.to_string()),
        only_known_rpc: matches.is_present("only_known_rpc"),
        max_genesis_archive_unpacked_size: value_t_or_exit!(
            matches,
            "max_genesis_archive_unpacked_size",
            u64
        ),
        incremental_snapshot_fetch: !matches.is_present("no_incremental_snapshots"),
    };

    /// 私有 rpc
    let private_rpc = matches.is_present("private_rpc");
    /// 不做端口检查
    let do_port_check = !matches.is_present("no_port_check");
    /// tpu 数据包报文合并等待秒数
    let tpu_coalesce = value_t!(matches, "tpu_coalesce_ms", u64)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TPU_COALESCE);

    // Canonicalize ledger path to avoid issues with symlink creation
    /// 账本路径
    let ledger_path = create_and_canonicalize_directories([&ledger_path])
        .unwrap_or_else(|err| {
            eprintln!(
                "Unable to access ledger path '{}': {err}",
                ledger_path.display(),
            );
            exit(1);
        })
        .pop()
        .unwrap();

    /// wal 恢复模式
    /// WAL 是一种用于确保数据库一致性的日志机制，常用于处理事务和保证数据的持久性。
    /// Solana 使用 WAL 来记录区块链中的重要状态和交易信息，以便在节点崩溃或重启时进行恢复。
    let recovery_mode = matches
        .value_of("wal_recovery_mode")
        .map(BlockstoreRecoveryMode::from);

    /// 限制账本大小
    let max_ledger_shreds = if matches.is_present("limit_ledger_size") {
        let limit_ledger_size = match matches.value_of("limit_ledger_size") {
            Some(_) => value_t_or_exit!(matches, "limit_ledger_size", u64),
            None => DEFAULT_MAX_LEDGER_SHREDS,
        };
        if limit_ledger_size < DEFAULT_MIN_MAX_LEDGER_SHREDS {
            eprintln!(
                "The provided --limit-ledger-size value was too small, the minimum value is \
                 {DEFAULT_MIN_MAX_LEDGER_SHREDS}"
            );
            exit(1);
        }
        Some(limit_ledger_size)
    } else {
        None
    };

    /// rockdbs 压缩算法和方式
    let column_options = LedgerColumnOptions {
        compression_type: match matches.value_of("rocksdb_ledger_compression") {
            None => BlockstoreCompressionType::default(),
            Some(ledger_compression_string) => match ledger_compression_string {
                "none" => BlockstoreCompressionType::None,
                "snappy" => BlockstoreCompressionType::Snappy,
                "lz4" => BlockstoreCompressionType::Lz4,
                "zlib" => BlockstoreCompressionType::Zlib,
                _ => panic!("Unsupported ledger_compression: {ledger_compression_string}"),
            },
        },
        rocks_perf_sample_interval: value_t_or_exit!(
            matches,
            "rocksdb_perf_sample_interval",
            usize
        ),
    };

    /// rockdbs 的相关配置，也就是块存储的相关配置
    let blockstore_options = BlockstoreOptions {
        recovery_mode,
        column_options,
        // The validator needs to open many files, check that the process has
        // permission to do so in order to fail quickly and give a direct error
        enforce_ulimit_nofile: true,
        // The validator needs primary (read/write)
        access_type: AccessType::Primary,
        num_rocksdb_compaction_threads: rocksdb_compaction_threads,
        num_rocksdb_flush_threads: rocksdb_flush_threads,
    };

    /// 账户缓存路径
    let accounts_hash_cache_path = matches
        .value_of("accounts_hash_cache_path")
        .map(Into::into)
        .unwrap_or_else(|| ledger_path.join(AccountsDb::DEFAULT_ACCOUNTS_HASH_CACHE_DIR));
    let accounts_hash_cache_path = create_and_canonicalize_directories([&accounts_hash_cache_path])
        .unwrap_or_else(|err| {
            eprintln!(
                "Unable to access accounts hash cache path '{}': {err}",
                accounts_hash_cache_path.display(),
            );
            exit(1);
        })
        .pop()
        .unwrap();

    /// debug key 带上这个的请求会详细打印日志
    let debug_keys: Option<Arc<HashSet<_>>> = if matches.is_present("debug_key") {
        Some(Arc::new(
            values_t_or_exit!(matches, "debug_key", Pubkey)
                .into_iter()
                .collect(),
        ))
    } else {
        None
    };

    /// 信任的验证器
    let known_validators = validators_set(
        &identity_keypair.pubkey(),
        &matches,
        "known_validators",
        "--known-validator",
    );
    /// 修复来源验证器
    let repair_validators = validators_set(
        &identity_keypair.pubkey(),
        &matches,
        "repair_validators",
        "--repair-validator",
    );
    /// 高优先级修复来源
    let repair_whitelist = validators_set(
        &identity_keypair.pubkey(),
        &matches,
        "repair_whitelist",
        "--repair-whitelist",
    );
    let repair_whitelist = Arc::new(RwLock::new(repair_whitelist.unwrap_or_default()));
    /// 八卦验证器
    let gossip_validators = validators_set(
        &identity_keypair.pubkey(),
        &matches,
        "gossip_validators",
        "--gossip-validator",
    );

    /// 总监听地址
    let bind_address = solana_net_utils::parse_host(matches.value_of("bind_address").unwrap())
        .expect("invalid bind_address");
    /// rpc 监听地址
    let rpc_bind_address = if matches.is_present("rpc_bind_address") {
        solana_net_utils::parse_host(matches.value_of("rpc_bind_address").unwrap())
            .expect("invalid rpc_bind_address")
    } else if private_rpc {
        solana_net_utils::parse_host("127.0.0.1").unwrap()
    } else {
        bind_address
    };

    /// 打印八卦连接debug信息的时间间隔，默认 120000 毫秒
    let contact_debug_interval = value_t_or_exit!(matches, "contact_debug_interval", u64);

    /// 账户索引相关的配置
    let account_indexes = process_account_indexes(&matches);

    /// 修复模式启动，不投票和发消息
    let restricted_repair_only_mode = matches.is_present("restricted_repair_only_mode");
    /// 压缩稀疏账户来减少空间占用
    let accounts_shrink_optimize_total_space =
        value_t_or_exit!(matches, "accounts_shrink_optimize_total_space", bool);
    /// tpu 使用 quic
    let tpu_use_quic = !matches.is_present("tpu_disable_quic");
    /// 投票使用 quic
    let vote_use_quic = value_t_or_exit!(matches, "vote_use_quic", bool);

    /// tpu 使用 udp
    let tpu_enable_udp = if matches.is_present("tpu_enable_udp") {
        true
    } else {
        DEFAULT_TPU_ENABLE_UDP
    };

    /// tpu 连接池大小
    let tpu_connection_pool_size = value_t_or_exit!(matches, "tpu_connection_pool_size", usize);
    /// tpu 最大连接数每地址每分钟
    let tpu_max_connections_per_ipaddr_per_minute =
        value_t_or_exit!(matches, "tpu_max_connections_per_ipaddr_per_minute", u64);

    /// 账户压缩比
    let shrink_ratio = value_t_or_exit!(matches, "accounts_shrink_ratio", f64);
    if !(0.0..=1.0).contains(&shrink_ratio) {
        eprintln!(
            "The specified account-shrink-ratio is invalid, it must be between 0. and 1.0 \
             inclusive: {shrink_ratio}"
        );
        exit(1);
    }

    let shrink_ratio = if accounts_shrink_optimize_total_space {
        AccountShrinkThreshold::TotalSpace { shrink_ratio }
    } else {
        AccountShrinkThreshold::IndividualStore { shrink_ratio }
    };
    /// 初始的八卦交流对象
    let entrypoint_addrs = values_t!(matches, "entrypoint", String)
        .unwrap_or_default()
        .into_iter()
        .map(|entrypoint| {
            solana_net_utils::parse_host_port(&entrypoint).unwrap_or_else(|e| {
                eprintln!("failed to parse entrypoint address: {e}");
                exit(1);
            })
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    for addr in &entrypoint_addrs {
        if !socket_addr_space.check(addr) {
            eprintln!("invalid entrypoint address: {addr}");
            exit(1);
        }
    }
    /// 预期碎屑版本
    // TODO: Once entrypoints are updated to return shred-version, this should
    // abort if it fails to obtain a shred-version, so that nodes always join
    // gossip with a valid shred-version. The code to adopt entrypoint shred
    // version can then be deleted from gossip and get_rpc_node above.
    let expected_shred_version = value_t!(matches, "expected_shred_version", u16)
        .ok()
        .or_else(|| get_cluster_shred_version(&entrypoint_addrs));

    /// 塔式共识存储，两种，FileTowerStorage 和 EtcdTowerStorage
    let tower_storage: Arc<dyn tower_storage::TowerStorage> =
        match value_t_or_exit!(matches, "tower_storage", String).as_str() {
            "file" => {
                let tower_path = value_t!(matches, "tower", PathBuf)
                    .ok()
                    .unwrap_or_else(|| ledger_path.clone());

                Arc::new(tower_storage::FileTowerStorage::new(tower_path))
            }
            "etcd" => {
                let endpoints = values_t_or_exit!(matches, "etcd_endpoint", String);
                let domain_name = value_t_or_exit!(matches, "etcd_domain_name", String);
                let ca_certificate_file = value_t_or_exit!(matches, "etcd_cacert_file", String);
                let identity_certificate_file = value_t_or_exit!(matches, "etcd_cert_file", String);
                let identity_private_key_file = value_t_or_exit!(matches, "etcd_key_file", String);

                let read = |file| {
                    fs::read(&file).unwrap_or_else(|err| {
                        eprintln!("Unable to read {file}: {err}");
                        exit(1)
                    })
                };

                let tls_config = tower_storage::EtcdTlsConfig {
                    domain_name,
                    ca_certificate: read(ca_certificate_file),
                    identity_certificate: read(identity_certificate_file),
                    identity_private_key: read(identity_private_key_file),
                };

                Arc::new(
                    tower_storage::EtcdTowerStorage::new(endpoints, Some(tls_config))
                        .unwrap_or_else(|err| {
                            eprintln!("Failed to connect to etcd: {err}");
                            exit(1);
                        }),
                )
            }
            _ => unreachable!(),
        };

    /// 账户索引配置
    let mut accounts_index_config = AccountsIndexConfig {
        started_from_validator: true, // this is the only place this is set
        num_flush_threads: Some(accounts_index_flush_threads),
        ..AccountsIndexConfig::default()
    };
    /// accounts_index_bins 是用于调整账户索引的 分桶数量，即将账户索引的存储结构分成若干个“桶”，每个桶对应一部分账户的哈希值范围。
    if let Ok(bins) = value_t!(matches, "accounts_index_bins", usize) {
        accounts_index_config.bins = Some(bins);
    }

    /// 账户索引大小限制
    accounts_index_config.index_limit_mb = if matches.is_present("disable_accounts_disk_index") {
        IndexLimitMb::InMemOnly
    } else {
        IndexLimitMb::Unlimited
    };

    {
        let mut accounts_index_paths: Vec<PathBuf> = if matches.is_present("accounts_index_path") {
            values_t_or_exit!(matches, "accounts_index_path", String)
                .into_iter()
                .map(PathBuf::from)
                .collect()
        } else {
            vec![]
        };
        if accounts_index_paths.is_empty() {
            accounts_index_paths = vec![ledger_path.join("accounts_index")];
        }
        accounts_index_config.drives = Some(accounts_index_paths);
    }

    const MB: usize = 1_024 * 1_024;
    /// 扫描结果大小限制
    accounts_index_config.scan_results_limit_bytes =
        value_t!(matches, "accounts_index_scan_results_limit_mb", usize)
            .ok()
            .map(|mb| mb * MB);

    /// 账户压缩存储路径
    let account_shrink_paths: Option<Vec<PathBuf>> =
        values_t!(matches, "account_shrink_path", String)
            .map(|shrink_paths| shrink_paths.into_iter().map(PathBuf::from).collect())
            .ok();
    let account_shrink_paths = account_shrink_paths.as_ref().map(|paths| {
        create_and_canonicalize_directories(paths).unwrap_or_else(|err| {
            eprintln!("Unable to access account shrink path: {err}");
            exit(1);
        })
    });
    let (account_shrink_run_paths, account_shrink_snapshot_paths) = account_shrink_paths
        .map(|paths| {
            create_all_accounts_run_and_snapshot_dirs(&paths).unwrap_or_else(|err| {
                eprintln!("Error: {err}");
                exit(1);
            })
        })
        .unzip();

    /// 读缓存限制字节
    let read_cache_limit_bytes = values_of::<usize>(&matches, "accounts_db_read_cache_limit_mb")
        .map(|limits| {
            match limits.len() {
                // we were given explicit low and high watermark values, so use them
                2 => (limits[0] * MB, limits[1] * MB),
                // we were given a single value, so use it for both low and high watermarks
                1 => (limits[0] * MB, limits[0] * MB),
                _ => {
                    // clap will enforce either one or two values is given
                    unreachable!(
                        "invalid number of values given to accounts-db-read-cache-limit-mb"
                    )
                }
            }
        });
    /// 古老账户设置
    let create_ancient_storage = matches
        .value_of("accounts_db_squash_storages_method")
        .map(|method| match method {
            "pack" => CreateAncientStorage::Pack,
            "append" => CreateAncientStorage::Append,
            _ => {
                // clap will enforce one of the above values is given
                unreachable!("invalid value given to accounts-db-squash-storages-method")
            }
        })
        .unwrap_or_default();
    /// 访问存储方法
    let storage_access = matches
        .value_of("accounts_db_access_storages_method")
        .map(|method| match method {
            "mmap" => StorageAccess::Mmap,
            "file" => StorageAccess::File,
            _ => {
                // clap will enforce one of the above values is given
                unreachable!("invalid value given to accounts-db-access-storages-method")
            }
        })
        .unwrap_or_default();

    /// 压缩账户的扫描过滤
    let scan_filter_for_shrinking = matches
        .value_of("accounts_db_scan_filter_for_shrinking")
        .map(|filter| match filter {
            "all" => ScanFilter::All,
            "only-abnormal" => ScanFilter::OnlyAbnormal,
            "only-abnormal-with-verify" => ScanFilter::OnlyAbnormalWithVerify,
            _ => {
                // clap will enforce one of the above values is given
                unreachable!("invalid value given to accounts_db_scan_filter_for_shrinking")
            }
        })
        .unwrap_or_default();

    /// rockdbs 相关设置
    let accounts_db_config = AccountsDbConfig {
        index: Some(accounts_index_config),
        account_indexes: Some(account_indexes.clone()),
        base_working_path: Some(ledger_path.clone()),
        accounts_hash_cache_path: Some(accounts_hash_cache_path),
        shrink_paths: account_shrink_run_paths,
        shrink_ratio,
        read_cache_limit_bytes,
        write_cache_limit_bytes: value_t!(matches, "accounts_db_cache_limit_mb", u64)
            .ok()
            .map(|mb| mb * MB as u64),
        ancient_append_vec_offset: value_t!(matches, "accounts_db_ancient_append_vecs", i64).ok(),
        ancient_storage_ideal_size: value_t!(
            matches,
            "accounts_db_ancient_storage_ideal_size",
            u64
        )
        .ok(),
        max_ancient_storages: value_t!(matches, "accounts_db_max_ancient_storages", usize).ok(),
        hash_calculation_pubkey_bins: value_t!(
            matches,
            "accounts_db_hash_calculation_pubkey_bins",
            usize
        )
        .ok(),
        exhaustively_verify_refcounts: matches.is_present("accounts_db_verify_refcounts"),
        create_ancient_storage,
        test_skip_rewrites_but_include_in_bank_hash: matches
            .is_present("accounts_db_test_skip_rewrites"),
        storage_access,
        scan_filter_for_shrinking,
        enable_experimental_accumulator_hash: matches
            .is_present("accounts_db_experimental_accumulator_hash"),
        verify_experimental_accumulator_hash: matches
            .is_present("accounts_db_verify_experimental_accumulator_hash"),
        snapshots_use_experimental_accumulator_hash: matches
            .is_present("accounts_db_snapshots_use_experimental_accumulator_hash"),
        num_clean_threads: Some(accounts_db_clean_threads),
        num_foreground_threads: Some(accounts_db_foreground_threads),
        num_hash_threads: Some(accounts_db_hash_threads),
        ..AccountsDbConfig::default()
    };

    let accounts_db_config = Some(accounts_db_config);

    /// geyser 插件配置
    let on_start_geyser_plugin_config_files = if matches.is_present("geyser_plugin_config") {
        Some(
            values_t_or_exit!(matches, "geyser_plugin_config", String)
                .into_iter()
                .map(PathBuf::from)
                .collect(),
        )
    } else {
        None
    };
    let starting_with_geyser_plugins: bool = on_start_geyser_plugin_config_files.is_some()
        || matches.is_present("geyser_plugin_always_enabled");

    /// rpc 谷歌大表配置
    let rpc_bigtable_config = if matches.is_present("enable_rpc_bigtable_ledger_storage")
        || matches.is_present("enable_bigtable_ledger_upload")
    {
        Some(RpcBigtableConfig {
            enable_bigtable_ledger_upload: matches.is_present("enable_bigtable_ledger_upload"),
            bigtable_instance_name: value_t_or_exit!(matches, "rpc_bigtable_instance_name", String),
            bigtable_app_profile_id: value_t_or_exit!(
                matches,
                "rpc_bigtable_app_profile_id",
                String
            ),
            timeout: value_t!(matches, "rpc_bigtable_timeout", u64)
                .ok()
                .map(Duration::from_secs),
            max_message_size: value_t_or_exit!(matches, "rpc_bigtable_max_message_size", usize),
        })
    } else {
        None
    };

    /// rpc 发送相关配置
    let rpc_send_retry_rate_ms = value_t_or_exit!(matches, "rpc_send_transaction_retry_ms", u64);
    let rpc_send_batch_size = value_t_or_exit!(matches, "rpc_send_transaction_batch_size", usize);
    let rpc_send_batch_send_rate_ms =
        value_t_or_exit!(matches, "rpc_send_transaction_batch_ms", u64);

    if rpc_send_batch_send_rate_ms > rpc_send_retry_rate_ms {
        eprintln!(
            "The specified rpc-send-batch-ms ({rpc_send_batch_send_rate_ms}) is invalid, it must \
             be <= rpc-send-retry-ms ({rpc_send_retry_rate_ms})"
        );
        exit(1);
    }

    /// 检查是否超速
    let tps = rpc_send_batch_size as u64 * MILLIS_PER_SECOND / rpc_send_batch_send_rate_ms;
    if tps > send_transaction_service::MAX_TRANSACTION_SENDS_PER_SECOND {
        eprintln!(
            "Either the specified rpc-send-batch-size ({}) or rpc-send-batch-ms ({}) is invalid, \
             'rpc-send-batch-size * 1000 / rpc-send-batch-ms' must be smaller than ({}) .",
            rpc_send_batch_size,
            rpc_send_batch_send_rate_ms,
            send_transaction_service::MAX_TRANSACTION_SENDS_PER_SECOND
        );
        exit(1);
    }
    /// 要转发到的tpu对等体配置
    let rpc_send_transaction_tpu_peers = matches
        .values_of("rpc_send_transaction_tpu_peer")
        .map(|values| {
            values
                .map(solana_net_utils::parse_host_port)
                .collect::<Result<Vec<SocketAddr>, String>>()
        })
        .transpose()
        .unwrap_or_else(|e| {
            eprintln!("failed to parse rpc send-transaction-service tpu peer address: {e}");
            exit(1);
        });
    let rpc_send_transaction_also_leader = matches.is_present("rpc_send_transaction_also_leader");
    let leader_forward_count =
        if rpc_send_transaction_tpu_peers.is_some() && !rpc_send_transaction_also_leader {
            // rpc-sts is configured to send only to specific tpu peers. disable leader forwards
            0
        } else {
            value_t_or_exit!(matches, "rpc_send_transaction_leader_forward_count", u64)
        };

    /// 是否开放全部 rpc api
    let full_api = matches.is_present("full_rpc_api");

    /// 验证器配置，到这里还只是部分配置
    /// 详见结构体注释
    let mut validator_config = ValidatorConfig {
        require_tower: matches.is_present("require_tower"),
        tower_storage,
        halt_at_slot: value_t!(matches, "dev_halt_at_slot", Slot).ok(),
        expected_genesis_hash: matches
            .value_of("expected_genesis_hash")
            .map(|s| Hash::from_str(s).unwrap()),
        expected_bank_hash: matches
            .value_of("expected_bank_hash")
            .map(|s| Hash::from_str(s).unwrap()),
        expected_shred_version,
        new_hard_forks: hardforks_of(&matches, "hard_forks"),
        rpc_config: JsonRpcConfig {
            enable_rpc_transaction_history: matches.is_present("enable_rpc_transaction_history"),
            enable_extended_tx_metadata_storage: matches.is_present("enable_cpi_and_log_storage")
                || matches.is_present("enable_extended_tx_metadata_storage"),
            rpc_bigtable_config,
            faucet_addr: matches.value_of("rpc_faucet_addr").map(|address| {
                solana_net_utils::parse_host_port(address).expect("failed to parse faucet address")
            }),
            full_api,
            max_multiple_accounts: Some(value_t_or_exit!(
                matches,
                "rpc_max_multiple_accounts",
                usize
            )),
            health_check_slot_distance: value_t_or_exit!(
                matches,
                "health_check_slot_distance",
                u64
            ),
            disable_health_check: false,
            rpc_threads: value_t_or_exit!(matches, "rpc_threads", usize),
            rpc_blocking_threads: value_t_or_exit!(matches, "rpc_blocking_threads", usize),
            rpc_niceness_adj: value_t_or_exit!(matches, "rpc_niceness_adj", i8),
            account_indexes: account_indexes.clone(),
            rpc_scan_and_fix_roots: matches.is_present("rpc_scan_and_fix_roots"),
            max_request_body_size: Some(value_t_or_exit!(
                matches,
                "rpc_max_request_body_size",
                usize
            )),
            skip_preflight_health_check: matches.is_present("skip_preflight_health_check"),
        },
        on_start_geyser_plugin_config_files,
        geyser_plugin_always_enabled: matches.is_present("geyser_plugin_always_enabled"),
        rpc_addrs: value_t!(matches, "rpc_port", u16).ok().map(|rpc_port| {
            (
                SocketAddr::new(rpc_bind_address, rpc_port),
                SocketAddr::new(rpc_bind_address, rpc_port + 1),
                // If additional ports are added, +2 needs to be skipped to avoid a conflict with
                // the websocket port (which is +2) in web3.js This odd port shifting is tracked at
                // https://github.com/solana-labs/solana/issues/12250
            )
        }),
        pubsub_config: PubSubConfig {
            enable_block_subscription: matches.is_present("rpc_pubsub_enable_block_subscription"),
            enable_vote_subscription: matches.is_present("rpc_pubsub_enable_vote_subscription"),
            max_active_subscriptions: value_t_or_exit!(
                matches,
                "rpc_pubsub_max_active_subscriptions",
                usize
            ),
            queue_capacity_items: value_t_or_exit!(
                matches,
                "rpc_pubsub_queue_capacity_items",
                usize
            ),
            queue_capacity_bytes: value_t_or_exit!(
                matches,
                "rpc_pubsub_queue_capacity_bytes",
                usize
            ),
            worker_threads: value_t_or_exit!(matches, "rpc_pubsub_worker_threads", usize),
            notification_threads: value_t!(matches, "rpc_pubsub_notification_threads", usize)
                .ok()
                .and_then(NonZeroUsize::new),
        },
        voting_disabled: matches.is_present("no_voting") || restricted_repair_only_mode,
        wait_for_supermajority: value_t!(matches, "wait_for_supermajority", Slot).ok(),
        known_validators,
        repair_validators,
        repair_whitelist,
        gossip_validators,
        max_ledger_shreds,
        blockstore_options,
        run_verification: !(matches.is_present("skip_poh_verify")
            || matches.is_present("skip_startup_ledger_verification")),
        debug_keys,
        contact_debug_interval,
        send_transaction_service_config: send_transaction_service::Config {
            retry_rate_ms: rpc_send_retry_rate_ms,
            leader_forward_count,
            default_max_retries: value_t!(
                matches,
                "rpc_send_transaction_default_max_retries",
                usize
            )
            .ok(),
            service_max_retries: value_t_or_exit!(
                matches,
                "rpc_send_transaction_service_max_retries",
                usize
            ),
            batch_send_rate_ms: rpc_send_batch_send_rate_ms,
            batch_size: rpc_send_batch_size,
            retry_pool_max_size: value_t_or_exit!(
                matches,
                "rpc_send_transaction_retry_pool_max_size",
                usize
            ),
            tpu_peers: rpc_send_transaction_tpu_peers,
        },
        no_poh_speed_test: matches.is_present("no_poh_speed_test"),
        no_os_memory_stats_reporting: matches.is_present("no_os_memory_stats_reporting"),
        no_os_network_stats_reporting: matches.is_present("no_os_network_stats_reporting"),
        no_os_cpu_stats_reporting: matches.is_present("no_os_cpu_stats_reporting"),
        no_os_disk_stats_reporting: matches.is_present("no_os_disk_stats_reporting"),
        poh_pinned_cpu_core: value_of(&matches, "poh_pinned_cpu_core")
            .unwrap_or(poh_service::DEFAULT_PINNED_CPU_CORE),
        poh_hashes_per_batch: value_of(&matches, "poh_hashes_per_batch")
            .unwrap_or(poh_service::DEFAULT_HASHES_PER_BATCH),
        process_ledger_before_services: matches.is_present("process_ledger_before_services"),
        accounts_db_test_hash_calculation: matches.is_present("accounts_db_test_hash_calculation"),
        accounts_db_config,
        accounts_db_skip_shrink: true,
        accounts_db_force_initial_clean: matches.is_present("no_skip_initial_accounts_db_clean"),
        tpu_coalesce,
        no_wait_for_vote_to_start_leader: matches.is_present("no_wait_for_vote_to_start_leader"),
        runtime_config: RuntimeConfig {
            log_messages_bytes_limit: value_of(&matches, "log_messages_bytes_limit"),
            ..RuntimeConfig::default()
        },
        staked_nodes_overrides: staked_nodes_overrides.clone(),
        use_snapshot_archives_at_startup: value_t_or_exit!(
            matches,
            use_snapshot_archives_at_startup::cli::NAME,
            UseSnapshotArchivesAtStartup
        ),
        ip_echo_server_threads,
        rayon_global_threads,
        replay_forks_threads,
        replay_transactions_threads,
        tvu_shred_sigverify_threads: tvu_sigverify_threads,
        delay_leader_block_for_pending_fork: matches
            .is_present("delay_leader_block_for_pending_fork"),
        wen_restart_proto_path: value_t!(matches, "wen_restart", PathBuf).ok(),
        wen_restart_coordinator: value_t!(matches, "wen_restart_coordinator", Pubkey).ok(),
        ..ValidatorConfig::default()
    };

    /// 投票账户
    let vote_account = pubkey_of(&matches, "vote_account").unwrap_or_else(|| {
        if !validator_config.voting_disabled {
            warn!("--vote-account not specified, validator will not vote");
            validator_config.voting_disabled = true;
        }
        Keypair::new().pubkey()
    });

    /// 动态端口范围
    let dynamic_port_range =
        solana_net_utils::parse_port_range(matches.value_of("dynamic_port_range").unwrap())
            .expect("invalid dynamic_port_range");

    /// 账户路径
    let account_paths: Vec<PathBuf> =
        if let Ok(account_paths) = values_t!(matches, "account_paths", String) {
            account_paths
                .join(",")
                .split(',')
                .map(PathBuf::from)
                .collect()
        } else {
            vec![ledger_path.join("accounts")]
        };
    let account_paths = create_and_canonicalize_directories(account_paths).unwrap_or_else(|err| {
        eprintln!("Unable to access account path: {err}");
        exit(1);
    });

    /// 创建账户的运行和快照路径
    let (account_run_paths, account_snapshot_paths) =
        create_all_accounts_run_and_snapshot_dirs(&account_paths).unwrap_or_else(|err| {
            eprintln!("Error: {err}");
            exit(1);
        });

    // From now on, use run/ paths in the same way as the previous account_paths.
    /// 设置账户路径
    validator_config.account_paths = account_run_paths;

    // These snapshot paths are only used for initial clean up, add in shrink paths if they exist.
    /// 设置账户快照路径
    validator_config.account_snapshot_paths =
        if let Some(account_shrink_snapshot_paths) = account_shrink_snapshot_paths {
            account_snapshot_paths
                .into_iter()
                .chain(account_shrink_snapshot_paths)
                .collect()
        } else {
            account_snapshot_paths
        };

    /// 最长本地快照年龄
    let maximum_local_snapshot_age = value_t_or_exit!(matches, "maximum_local_snapshot_age", u64);
    /// 最大全快照保留
    let maximum_full_snapshot_archives_to_retain =
        value_t_or_exit!(matches, "maximum_full_snapshots_to_retain", NonZeroUsize);
    /// 最大增量快照保留
    let maximum_incremental_snapshot_archives_to_retain = value_t_or_exit!(
        matches,
        "maximum_incremental_snapshots_to_retain",
        NonZeroUsize
    );
    /// 快照拍摄线程优先级
    let snapshot_packager_niceness_adj =
        value_t_or_exit!(matches, "snapshot_packager_niceness_adj", i8);
    /// 最小快照下载速度
    let minimal_snapshot_download_speed =
        value_t_or_exit!(matches, "minimal_snapshot_download_speed", f32);
    /// 最大快照下载中断
    let maximum_snapshot_download_abort =
        value_t_or_exit!(matches, "maximum_snapshot_download_abort", u64);

    /// 创建快照路径
    let snapshots_dir = if let Some(snapshots) = matches.value_of("snapshots") {
        Path::new(snapshots)
    } else {
        &ledger_path
    };
    let snapshots_dir = create_and_canonicalize_directory(snapshots_dir).unwrap_or_else(|err| {
        eprintln!(
            "Failed to create snapshots directory '{}': {err}",
            snapshots_dir.display(),
        );
        exit(1);
    });

    /// 账户和快照路径不能是同一个
    if account_paths
        .iter()
        .any(|account_path| account_path == &snapshots_dir)
    {
        eprintln!(
            "Failed: The --accounts and --snapshots paths must be unique since they \
             both create 'snapshots' subdirectories, otherwise there may be collisions",
        );
        exit(1);
    }

    /// 创建银行快照路径
    let bank_snapshots_dir = snapshots_dir.join("snapshots");
    fs::create_dir_all(&bank_snapshots_dir).unwrap_or_else(|err| {
        eprintln!(
            "Failed to create bank snapshots directory '{}': {err}",
            bank_snapshots_dir.display(),
        );
        exit(1);
    });

    /// 创建全快照归档路径
    let full_snapshot_archives_dir =
        if let Some(full_snapshot_archive_path) = matches.value_of("full_snapshot_archive_path") {
            PathBuf::from(full_snapshot_archive_path)
        } else {
            snapshots_dir.clone()
        };
    fs::create_dir_all(&full_snapshot_archives_dir).unwrap_or_else(|err| {
        eprintln!(
            "Failed to create full snapshot archives directory '{}': {err}",
            full_snapshot_archives_dir.display(),
        );
        exit(1);
    });

    /// 创建增量快照存档路径
    let incremental_snapshot_archives_dir = if let Some(incremental_snapshot_archive_path) =
        matches.value_of("incremental_snapshot_archive_path")
    {
        PathBuf::from(incremental_snapshot_archive_path)
    } else {
        snapshots_dir.clone()
    };
    fs::create_dir_all(&incremental_snapshot_archives_dir).unwrap_or_else(|err| {
        eprintln!(
            "Failed to create incremental snapshot archives directory '{}': {err}",
            incremental_snapshot_archives_dir.display(),
        );
        exit(1);
    });

    /// 归档格式
    let archive_format = {
        let archive_format_str = value_t_or_exit!(matches, "snapshot_archive_format", String);
        ArchiveFormat::from_cli_arg(&archive_format_str)
            .unwrap_or_else(|| panic!("Archive format not recognized: {archive_format_str}"))
    };

    /// 快照版本
    let snapshot_version =
        matches
            .value_of("snapshot_version")
            .map_or(SnapshotVersion::default(), |s| {
                s.parse::<SnapshotVersion>().unwrap_or_else(|err| {
                    eprintln!("Error: {err}");
                    exit(1)
                })
            });

    /// 快照归档间隔
    let (full_snapshot_archive_interval_slots, incremental_snapshot_archive_interval_slots) = match (
        !matches.is_present("no_incremental_snapshots"),
        value_t_or_exit!(matches, "snapshot_interval_slots", u64),
    ) {
        (_, 0) => {
            // snapshots are disabled
            (
                DISABLED_SNAPSHOT_ARCHIVE_INTERVAL,
                DISABLED_SNAPSHOT_ARCHIVE_INTERVAL,
            )
        }
        (true, incremental_snapshot_interval_slots) => {
            // incremental snapshots are enabled
            // use --snapshot-interval-slots for the incremental snapshot interval
            (
                value_t_or_exit!(matches, "full_snapshot_interval_slots", u64),
                incremental_snapshot_interval_slots,
            )
        }
        (false, full_snapshot_interval_slots) => {
            // incremental snapshots are *disabled*
            // use --snapshot-interval-slots for the *full* snapshot interval
            // also warn if --full-snapshot-interval-slots was specified
            if matches.occurrences_of("full_snapshot_interval_slots") > 0 {
                warn!(
                    "Incremental snapshots are disabled, yet --full-snapshot-interval-slots was specified! \
                     Note that --full-snapshot-interval-slots is *ignored* when incremental snapshots are disabled. \
                     Use --snapshot-interval-slots instead.",
                );
            }
            (
                full_snapshot_interval_slots,
                DISABLED_SNAPSHOT_ARCHIVE_INTERVAL,
            )
        }
    };

    /// 设置快照配置，详见结构体注释
    validator_config.snapshot_config = SnapshotConfig {
        usage: if full_snapshot_archive_interval_slots == DISABLED_SNAPSHOT_ARCHIVE_INTERVAL {
            SnapshotUsage::LoadOnly
        } else {
            SnapshotUsage::LoadAndGenerate
        },
        full_snapshot_archive_interval_slots,
        incremental_snapshot_archive_interval_slots,
        bank_snapshots_dir,
        full_snapshot_archives_dir: full_snapshot_archives_dir.clone(),
        incremental_snapshot_archives_dir: incremental_snapshot_archives_dir.clone(),
        archive_format,
        snapshot_version,
        maximum_full_snapshot_archives_to_retain,
        maximum_incremental_snapshot_archives_to_retain,
        accounts_hash_debug_verify: validator_config.accounts_db_test_hash_calculation,
        packager_thread_niceness_adj: snapshot_packager_niceness_adj,
    };

    // The accounts hash interval shall match the snapshot interval
    /// 账户哈希时间间隔
    validator_config.accounts_hash_interval_slots = std::cmp::min(
        full_snapshot_archive_interval_slots,
        incremental_snapshot_archive_interval_slots,
    );

    /// 打印归档时间间隔
    info!(
        "Snapshot configuration: full snapshot interval: {} slots, incremental snapshot interval: {} slots",
        if full_snapshot_archive_interval_slots == DISABLED_SNAPSHOT_ARCHIVE_INTERVAL {
            "disabled".to_string()
        } else {
            full_snapshot_archive_interval_slots.to_string()
        },
        if incremental_snapshot_archive_interval_slots == DISABLED_SNAPSHOT_ARCHIVE_INTERVAL {
            "disabled".to_string()
        } else {
            incremental_snapshot_archive_interval_slots.to_string()
        },
    );

    /// 验证快照配置有效性
    /// 全快照拍摄时间是增量快照拍摄时间的整数倍，增量快照的拍摄时间是账户哈希的整数倍
    if !is_snapshot_config_valid(
        &validator_config.snapshot_config,
        validator_config.accounts_hash_interval_slots,
    ) {
        eprintln!(
            "Invalid snapshot configuration provided: snapshot intervals are incompatible. \
             \n\t- full snapshot interval MUST be a multiple of incremental snapshot interval \
             (if enabled) \
             \n\t- full snapshot interval MUST be larger than incremental snapshot interval \
             (if enabled)",
        );
        exit(1);
    }

    /// 解析并配置banking trace的目录大小限制传参
    configure_banking_trace_dir_byte_limit(&mut validator_config, &matches);
    /// 块验证方法
    validator_config.block_verification_method = value_t!(
        matches,
        "block_verification_method",
        BlockVerificationMethod
    )
    .unwrap_or_default();
    /// 块生产方法
    validator_config.block_production_method = value_t!(
        matches, // comment to align formatting...
        "block_production_method",
        BlockProductionMethod
    )
    .unwrap_or_default();
    /// 配置块生产转发，当使用覆盖节点质押量时
    validator_config.enable_block_production_forwarding = staked_nodes_overrides_path.is_some();
    /// 调度器线程数
    validator_config.unified_scheduler_handler_threads =
        value_t!(matches, "unified_scheduler_handler_threads", usize).ok();

    /// 公共rpc地址
    let public_rpc_addr = matches.value_of("public_rpc_addr").map(|addr| {
        solana_net_utils::parse_host_port(addr).unwrap_or_else(|e| {
            eprintln!("failed to parse public rpc address: {e}");
            exit(1);
        })
    });

    /// 检查网络配置
    /// const INTERESTING_LIMITS: &[(&str, InterestingLimit)] = &[
    ///     ("net.core.rmem_max", InterestingLimit::Recommend(134217728)),
    ///     ("net.core.wmem_max", InterestingLimit::Recommend(134217728)),
    ///     ("vm.max_map_count", InterestingLimit::Recommend(1000000)),
    ///     ("net.core.optmem_max", InterestingLimit::QueryOnly),
    ///     ("net.core.netdev_max_backlog", InterestingLimit::QueryOnly),
    /// ];
    if !matches.is_present("no_os_network_limits_test") {
        if SystemMonitorService::check_os_network_limits() {
            info!("OS network limits test passed.");
        } else {
            eprintln!("OS network limit test failed. See: https://docs.solanalabs.com/operations/guides/validator-start#system-tuning");
            exit(1);
        }
    }

    /// 锁住账本
    let mut ledger_lock = ledger_lockfile(&ledger_path);
    let _ledger_write_guard = lock_ledger(&ledger_path, &mut ledger_lock);

    /// 新建启动进度
    let start_progress = Arc::new(RwLock::new(ValidatorStartProgress::default()));
    let admin_service_post_init = Arc::new(RwLock::new(None));
    /// rpc 到插件管理器 channel
    let (rpc_to_plugin_manager_sender, rpc_to_plugin_manager_receiver) =
        if starting_with_geyser_plugins {
            let (sender, receiver) = unbounded();
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
    /// 启动本地 admin rpc 服务
    admin_rpc_service::run(
        &ledger_path,
        admin_rpc_service::AdminRpcRequestMetadata {
            rpc_addr: validator_config.rpc_addrs.map(|(rpc_addr, _)| rpc_addr),
            start_time: std::time::SystemTime::now(),
            validator_exit: validator_config.validator_exit.clone(),
            start_progress: start_progress.clone(),
            authorized_voter_keypairs: authorized_voter_keypairs.clone(),
            post_init: admin_service_post_init.clone(),
            tower_storage: validator_config.tower_storage.clone(),
            staked_nodes_overrides,
            rpc_to_plugin_manager_sender,
        },
    );

    let gossip_host: IpAddr = matches
        .value_of("gossip_host")
        .map(|gossip_host| {
            solana_net_utils::parse_host(gossip_host).unwrap_or_else(|err| {
                eprintln!("Failed to parse --gossip-host: {err}");
                exit(1);
            })
        })
        .unwrap_or_else(|| {
            if !entrypoint_addrs.is_empty() {
                let mut order: Vec<_> = (0..entrypoint_addrs.len()).collect();
                order.shuffle(&mut thread_rng());

                let gossip_host = order.into_iter().find_map(|i| {
                    let entrypoint_addr = &entrypoint_addrs[i];
                    info!(
                        "Contacting {} to determine the validator's public IP address",
                        entrypoint_addr
                    );
                    solana_net_utils::get_public_ip_addr(entrypoint_addr).map_or_else(
                        |err| {
                            eprintln!(
                                "Failed to contact cluster entrypoint {entrypoint_addr}: {err}"
                            );
                            None
                        },
                        Some,
                    )
                });

                gossip_host.unwrap_or_else(|| {
                    eprintln!("Unable to determine the validator's public IP address");
                    exit(1);
                })
            } else {
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            }
        });

    let gossip_addr = SocketAddr::new(
        gossip_host,
        value_t!(matches, "gossip_port", u16).unwrap_or_else(|_| {
            solana_net_utils::find_available_port_in_range(bind_address, (0, 1)).unwrap_or_else(
                |err| {
                    eprintln!("Unable to find an available gossip port: {err}");
                    exit(1);
                },
            )
        }),
    );

    let public_tpu_addr = matches.value_of("public_tpu_addr").map(|public_tpu_addr| {
        solana_net_utils::parse_host_port(public_tpu_addr).unwrap_or_else(|err| {
            eprintln!("Failed to parse --public-tpu-address: {err}");
            exit(1);
        })
    });

    let public_tpu_forwards_addr =
        matches
            .value_of("public_tpu_forwards_addr")
            .map(|public_tpu_forwards_addr| {
                solana_net_utils::parse_host_port(public_tpu_forwards_addr).unwrap_or_else(|err| {
                    eprintln!("Failed to parse --public-tpu-forwards-address: {err}");
                    exit(1);
                })
            });

    let num_quic_endpoints = value_t_or_exit!(matches, "num_quic_endpoints", NonZeroUsize);
    let node_config = NodeConfig {
        gossip_addr,
        port_range: dynamic_port_range,
        bind_ip_addr: bind_address,
        public_tpu_addr,
        public_tpu_forwards_addr,
        num_tvu_sockets: tvu_receive_threads,
        num_quic_endpoints,
    };

    let cluster_entrypoints = entrypoint_addrs
        .iter()
        .map(ContactInfo::new_gossip_entry_point)
        .collect::<Vec<_>>();

    let mut node = Node::new_with_external_ip(&identity_keypair.pubkey(), node_config);

    if restricted_repair_only_mode {
        if validator_config.wen_restart_proto_path.is_some() {
            error!("--restricted-repair-only-mode is not compatible with --wen_restart");
            exit(1);
        }

        // When in --restricted_repair_only_mode is enabled only the gossip and repair ports
        // need to be reachable by the entrypoint to respond to gossip pull requests and repair
        // requests initiated by the node.  All other ports are unused.
        node.info.remove_tpu();
        node.info.remove_tpu_forwards();
        node.info.remove_tvu();
        node.info.remove_serve_repair();

        // A node in this configuration shouldn't be an entrypoint to other nodes
        node.sockets.ip_echo = None;
    }

    if !private_rpc {
        macro_rules! set_socket {
            ($method:ident, $addr:expr, $name:literal) => {
                node.info.$method($addr).expect(&format!(
                    "Operator must spin up node with valid {} address",
                    $name
                ))
            };
        }
        if let Some(public_rpc_addr) = public_rpc_addr {
            set_socket!(set_rpc, public_rpc_addr, "RPC");
            set_socket!(set_rpc_pubsub, public_rpc_addr, "RPC-pubsub");
        } else if let Some((rpc_addr, rpc_pubsub_addr)) = validator_config.rpc_addrs {
            let addr = node
                .info
                .gossip()
                .expect("Operator must spin up node with valid gossip address")
                .ip();
            set_socket!(set_rpc, (addr, rpc_addr.port()), "RPC");
            set_socket!(set_rpc_pubsub, (addr, rpc_pubsub_addr.port()), "RPC-pubsub");
        }
    }

    solana_metrics::set_host_id(identity_keypair.pubkey().to_string());
    solana_metrics::set_panic_hook("validator", Some(String::from(solana_version)));
    solana_entry::entry::init_poh();
    snapshot_utils::remove_tmp_snapshot_archives(&full_snapshot_archives_dir);
    snapshot_utils::remove_tmp_snapshot_archives(&incremental_snapshot_archives_dir);

    let identity_keypair = Arc::new(identity_keypair);

    let should_check_duplicate_instance = true;
    if !cluster_entrypoints.is_empty() {
        bootstrap::rpc_bootstrap(
            &node,
            &identity_keypair,
            &ledger_path,
            &full_snapshot_archives_dir,
            &incremental_snapshot_archives_dir,
            &vote_account,
            authorized_voter_keypairs.clone(),
            &cluster_entrypoints,
            &mut validator_config,
            rpc_bootstrap_config,
            do_port_check,
            use_progress_bar,
            maximum_local_snapshot_age,
            should_check_duplicate_instance,
            &start_progress,
            minimal_snapshot_download_speed,
            maximum_snapshot_download_abort,
            socket_addr_space,
        );
        *start_progress.write().unwrap() = ValidatorStartProgress::Initializing;
    }

    if operation == Operation::Initialize {
        info!("Validator ledger initialization complete");
        return;
    }

    // Bootstrap code above pushes a contact-info with more recent timestamp to
    // gossip. If the node is staked the contact-info lingers in gossip causing
    // false duplicate nodes error.
    // Below line refreshes the timestamp on contact-info so that it overrides
    // the one pushed by bootstrap.
    node.info.hot_swap_pubkey(identity_keypair.pubkey());

    let validator = match Validator::new(
        node,
        identity_keypair,
        &ledger_path,
        &vote_account,
        authorized_voter_keypairs,
        cluster_entrypoints,
        &validator_config,
        should_check_duplicate_instance,
        rpc_to_plugin_manager_receiver,
        start_progress,
        socket_addr_space,
        ValidatorTpuConfig {
            use_quic: tpu_use_quic,
            vote_use_quic,
            tpu_connection_pool_size,
            tpu_enable_udp,
            tpu_max_connections_per_ipaddr_per_minute,
        },
        admin_service_post_init,
    ) {
        Ok(validator) => validator,
        Err(err) => match err.downcast_ref() {
            Some(ValidatorError::WenRestartFinished) => {
                error!("Please remove --wen_restart and use --wait_for_supermajority as instructed above");
                exit(200);
            }
            _ => {
                error!("Failed to start validator: {:?}", err);
                exit(1);
            }
        },
    };

    if let Some(filename) = init_complete_file {
        File::create(filename).unwrap_or_else(|_| {
            error!("Unable to create: {}", filename);
            exit(1);
        });
    }
    info!("Validator initialized");
    validator.join();
    info!("Validator exiting..");
}

/// 解析账户索引相关参数
fn process_account_indexes(matches: &ArgMatches) -> AccountSecondaryIndexes {
    /// 要启用的账户索引种类
    let account_indexes: HashSet<AccountIndex> = matches
        .values_of("account_indexes")
        .unwrap_or_default()
        .map(|value| match value {
            "program-id" => AccountIndex::ProgramId,
            "spl-token-mint" => AccountIndex::SplTokenMint,
            "spl-token-owner" => AccountIndex::SplTokenOwner,
            _ => unreachable!(),
        })
        .collect();

    /// 只包含
    let account_indexes_include_keys: HashSet<Pubkey> =
        values_t!(matches, "account_index_include_key", Pubkey)
            .unwrap_or_default()
            .iter()
            .cloned()
            .collect();

    /// 排除
    let account_indexes_exclude_keys: HashSet<Pubkey> =
        values_t!(matches, "account_index_exclude_key", Pubkey)
            .unwrap_or_default()
            .iter()
            .cloned()
            .collect();

    let exclude_keys = !account_indexes_exclude_keys.is_empty();
    let include_keys = !account_indexes_include_keys.is_empty();

    /// 整合包含和排除
    let keys = if !account_indexes.is_empty() && (exclude_keys || include_keys) {
        let account_indexes_keys = AccountSecondaryIndexesIncludeExclude {
            exclude: exclude_keys,
            keys: if exclude_keys {
                account_indexes_exclude_keys
            } else {
                account_indexes_include_keys
            },
        };
        Some(account_indexes_keys)
    } else {
        None
    };

    AccountSecondaryIndexes {
        keys,
        indexes: account_indexes,
    }
}
