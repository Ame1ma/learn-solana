use {
    crate::{
        admin_rpc_service, format_name_value, new_spinner_progress_bar, println_name_value,
        ProgressBar,
    },
    console::style,
    solana_core::validator::ValidatorStartProgress,
    solana_rpc_client::rpc_client::RpcClient,
    solana_rpc_client_api::{client_error, request, response::RpcContactInfo},
    solana_sdk::{
        clock::Slot, commitment_config::CommitmentConfig, exit::Exit, native_token::Sol,
        pubkey::Pubkey,
    },
    std::{
        io,
        net::SocketAddr,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread,
        time::{Duration, SystemTime},
    },
};

//! 监控仪表板

/// 监控仪表板
pub struct Dashboard {
    /// 进度条状态
    progress_bar: ProgressBar,
    /// 账本路径
    ledger_path: PathBuf,
    /// 共享退出标志
    exit: Arc<AtomicBool>,
}

impl Dashboard {
    /// 新建监控仪表板
    pub fn new(
        ledger_path: &Path,
        log_path: Option<&Path>,
        validator_exit: Option<&mut Exit>,
    ) -> Result<Self, io::Error> {
        // 打印账本路径
        println_name_value("Ledger location:", &format!("{}", ledger_path.display()));
        // 如果有 log 路径就打印 log 路径
        if let Some(log_path) = log_path {
            println_name_value("Log:", &format!("{}", log_path.display()));
        }

        /// 新建进度条
        let progress_bar = new_spinner_progress_bar();
        /// 显示信息，初始化中
        progress_bar.set_message("Initializing...");

        /// 注册验证器退出标志，当验证器退出时退出
        let exit = Arc::new(AtomicBool::new(false));
        if let Some(validator_exit) = validator_exit {
            let exit = exit.clone();
            validator_exit.register_exit(Box::new(move || exit.store(true, Ordering::Relaxed)));
        }

        Ok(Self {
            exit,
            ledger_path: ledger_path.to_path_buf(),
            progress_bar,
        })
    }

    /// 运行监控仪表板
    pub fn run(self, refresh_interval: Duration) {
        /// 解构
        let Self {
            exit,
            ledger_path,
            progress_bar,
            ..
        } = self;
        /// 删除旧进度条
        drop(progress_bar);

        /// 给 admin rpc 新建 tokio 单线程运行时，因为 rpc 是 async 函数
        let runtime = admin_rpc_service::runtime();
        /// 检查共享退出标志，只要没有退出就一直循环
        /// 一般在多线程情况下，访问原子变量，没有特别的需求就可以用 Ordering::Relaxed，不约束重排
        while !exit.load(Ordering::Relaxed) {
            /// 新建进度条
            let progress_bar = new_spinner_progress_bar();
            /// 设置消息
            progress_bar.set_message("Connecting...");

            /// 等待验证器启动，拿到正式 rpc 地址，和启动时间
            let Some((rpc_addr, start_time)) = runtime.block_on(wait_for_validator_startup(
                &ledger_path,
                &exit,
                progress_bar,
                refresh_interval,
            )) else {
                /// 不行就重来
                continue;
            };

            /// 新建正式 rpc 客户端
            let rpc_client = RpcClient::new_socket(rpc_addr);
            /// 获取 验证器公钥
            let mut identity = match rpc_client.get_identity() {
                Ok(identity) => identity,
                Err(err) => {
                    println!("Failed to get validator identity over RPC: {err}");
                    continue;
                }
            };
            /// 打印 验证器公钥
            println_name_value("Identity:", &identity.to_string());

            /// 获取和打印 创世区块哈希
            if let Ok(genesis_hash) = rpc_client.get_genesis_hash() {
                println_name_value("Genesis Hash:", &genesis_hash.to_string());
            }

            /// 获取并打印 联系方式信息
            if let Some(contact_info) = get_contact_info(&rpc_client, &identity) {
                /// 大版本
                println_name_value(
                    "Version:",
                    &contact_info.version.unwrap_or_else(|| "?".to_string()),
                );
                /// 小版本
                if let Some(shred_version) = contact_info.shred_version {
                    println_name_value("Shred Version:", &shred_version.to_string());
                }
                /// 八卦地址
                if let Some(gossip) = contact_info.gossip {
                    println_name_value("Gossip Address:", &gossip.to_string());
                }
                /// tpu 地址
                if let Some(tpu) = contact_info.tpu {
                    println_name_value("TPU Address:", &tpu.to_string());
                }
                /// rpc 地址
                if let Some(rpc) = contact_info.rpc {
                    println_name_value("JSON RPC URL:", &format!("http://{rpc}"));
                }
                /// websocket 地址
                if let Some(pubsub) = contact_info.pubsub {
                    println_name_value("WebSocket PubSub URL:", &format!("ws://{pubsub}"));
                }
            }

            /// 新建进度条
            let progress_bar = new_spinner_progress_bar();
            /// 快照时隙信息
            let mut snapshot_slot_info = None;
            /// 循环监控
            for i in 0.. {
                /// 如果验证器退出标志为退出则退出
                if exit.load(Ordering::Relaxed) {
                    break;
                }
                /// 每10次循环取一次最高时隙的快照
                if i % 10 == 0 {
                    snapshot_slot_info = rpc_client.get_highest_snapshot_slot().ok();
                }

                /// 公钥变更则打印变更信息
                let new_identity = rpc_client.get_identity().unwrap_or(identity);
                if identity != new_identity {
                    identity = new_identity;
                    progress_bar.println(format_name_value("Identity:", &identity.to_string()));
                }

                /// 获取验证器实时状态并打印
                match get_validator_stats(&rpc_client, &identity) {
                    Ok((
                        processed_slot,
                        confirmed_slot,
                        finalized_slot,
                        transaction_count,
                        identity_balance,
                        health,
                    )) => {
                        /// 已运行时间
                        let uptime = {
                            let uptime =
                                chrono::Duration::from_std(start_time.elapsed().unwrap()).unwrap();

                            format!(
                                "{:02}:{:02}:{:02} ",
                                uptime.num_hours(),
                                uptime.num_minutes() % 60,
                                uptime.num_seconds() % 60
                            )
                        };

                        /// 打印 已运行时间，多少时隙已被处理，多少时隙已被验证，多少时隙已被终结，多少时隙被全快照，多少时隙被增量快照
                        /// 在 Solana 网络中，Full Snapshot 和 Incremental Snapshot 是两种用于保存区块链状态的快照，主要用于加速节点同步，减少从创世区块开始重放所有交易所需的时间。
                        /// 1. Full Snapshot
                        /// 定义：完整快照包含区块链在某个特定 slot（区块高度）上的完整状态，包括所有账户的数据和状态。
                        /// 特点：
                        /// 包含整个链上状态（账户余额、程序状态等）。
                        /// 通常体积较大，因为它保存了区块链的全部状态。
                        /// 新节点可以从完整快照开始加载，无需从创世块开始处理交易。
                        /// 作用：
                        /// 为新节点或长时间未同步的节点提供一个完整的初始状态。
                        /// 大幅减少节点启动和同步所需时间。
                        /// 2. Incremental Snapshot
                        /// 定义：增量快照保存的是自上一个完整快照之后，区块链状态的增量变化。
                        /// 特点：
                        /// 体积小，通常只包含最近一段时间的状态更新。
                        /// 增量快照是基于某个完整快照构建的。
                        /// 增量快照必须与其对应的完整快照一起使用才能还原完整的区块链状态。
                        /// 作用：
                        /// 减少存储和带宽消耗。
                        /// 节点可以通过应用增量快照快速更新到最新状态，而无需重新下载完整快照。
                        /// 3. 两者关系与节点同步流程
                        /// 同步过程：
                        /// 新节点下载最新的完整快照（Full Snapshot）。
                        /// 应用从完整快照生成时起的所有增量快照（Incremental Snapshots）。
                        /// 从最新的 slot 开始处理新的区块以保持与网络同步。
                        /// 优点：
                        /// Full Snapshot 提供了一个“基础状态”。
                        /// Incremental Snapshot 减少了完整快照的更新频率，节省存储空间和网络资源。
                        /// 4. 实际应用
                        /// 在运行 Solana 节点时：
                        /// 节点运营者可以配置节点定期生成完整快照和增量快照，以支持其他节点快速同步。
                        /// 官方提供的公共快照服务可以让普通用户轻松地启动节点，而无需手动生成这些快照。
                        /// 这些机制是为了优化 Solana 网络中节点同步效率的重要设计。
                        progress_bar.set_message(format!(
                            "{}{}| Processed Slot: {} | Confirmed Slot: {} | Finalized Slot: {} | \
                             Full Snapshot Slot: {} | Incremental Snapshot Slot: {} | \
                             Transactions: {} | {}",
                            uptime,
                            if health == "ok" {
                                "".to_string()
                            } else {
                                format!("| {} ", style(health).bold().red())
                            },
                            processed_slot,
                            confirmed_slot,
                            finalized_slot,
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
                            transaction_count,
                            identity_balance
                        ));
                        thread::sleep(refresh_interval);
                    }
                    /// 获取失败，打印连接异常
                    Err(err) => {
                        progress_bar.abandon_with_message(format!("RPC connection failure: {err}"));
                        break;
                    }
                }
            }
        }
    }
}

/// 等待验证器启动完成
async fn wait_for_validator_startup(
    ledger_path: &Path,
    exit: &AtomicBool,
    progress_bar: ProgressBar,
    refresh_interval: Duration,
) -> Option<(SocketAddr, SystemTime)> {
    let mut admin_client = None;
    loop {
        /// 设置了退出标志就退出
        if exit.load(Ordering::Relaxed) {
            return None;
        }

        /// 尝试连接到验证器，不行就重来
        if admin_client.is_none() {
            /// admin 客户端，通过 sockets 文件连接
            match admin_rpc_service::connect(ledger_path).await {
                Ok(new_admin_client) => admin_client = Some(new_admin_client),
                Err(err) => {
                    progress_bar.set_message(format!("Unable to connect to validator: {err}"));
                    thread::sleep(refresh_interval);
                    continue;
                }
            }
        }

        /// 发送 rpc 请求，获取 启动进度
        match admin_client.as_ref().unwrap().start_progress().await {
            Ok(start_progress) => {
                /// 已经开始运行了
                if start_progress == ValidatorStartProgress::Running {
                    let admin_client = admin_client.take().unwrap();

                    /// 获取验证器 rpc url 和启动时间
                    let validator_info = async move {
                        let rpc_addr = admin_client.rpc_addr().await?;
                        let start_time = admin_client.start_time().await?;
                        Ok::<_, jsonrpc_core_client::RpcError>((rpc_addr, start_time))
                    }
                    .await;
                    /// 返回 rpc url 和启动时间
                    match validator_info {
                        Ok((None, _)) => progress_bar.set_message("RPC service not available"),
                        Ok((Some(rpc_addr), start_time)) => return Some((rpc_addr, start_time)),
                        Err(err) => {
                            progress_bar
                                .set_message(format!("Failed to get validator info: {err}"));
                        }
                    }
                } else {
                    /// 在启动过程中，显示在进度条信息上
                    progress_bar.set_message(format!("Validator startup: {start_progress:?}..."));
                }
            }
            Err(err) => {
                /// 获取失败，重来
                admin_client = None;
                progress_bar.set_message(format!("Failed to get validator start progress: {err}"));
            }
        }
        /// 等待下一次循环
        thread::sleep(refresh_interval);
    }
}

fn get_contact_info(rpc_client: &RpcClient, identity: &Pubkey) -> Option<RpcContactInfo> {
    rpc_client
        .get_cluster_nodes()
        .ok()
        .unwrap_or_default()
        .into_iter()
        .find(|node| node.pubkey == identity.to_string())
}

fn get_validator_stats(
    rpc_client: &RpcClient,
    identity: &Pubkey,
) -> client_error::Result<(Slot, Slot, Slot, u64, Sol, String)> {
    let finalized_slot = rpc_client.get_slot_with_commitment(CommitmentConfig::finalized())?;
    let confirmed_slot = rpc_client.get_slot_with_commitment(CommitmentConfig::confirmed())?;
    let processed_slot = rpc_client.get_slot_with_commitment(CommitmentConfig::processed())?;
    let transaction_count =
        rpc_client.get_transaction_count_with_commitment(CommitmentConfig::processed())?;
    let identity_balance = rpc_client
        .get_balance_with_commitment(identity, CommitmentConfig::confirmed())?
        .value;

    let health = match rpc_client.get_health() {
        Ok(()) => "ok".to_string(),
        Err(err) => {
            if let client_error::ErrorKind::RpcError(request::RpcError::RpcResponseError {
                code: _,
                message: _,
                data:
                    request::RpcResponseErrorData::NodeUnhealthy {
                        num_slots_behind: Some(num_slots_behind),
                    },
            }) = &err.kind
            {
                format!("{num_slots_behind} slots behind")
            } else {
                "health unknown".to_string()
            }
        }
    };

    Ok((
        processed_slot,
        confirmed_slot,
        finalized_slot,
        transaction_count,
        Sol(identity_balance),
        health,
    ))
}
