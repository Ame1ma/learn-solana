use {
    clap::{
        crate_description, crate_name, App, AppSettings, Arg, ArgGroup, ArgMatches, SubCommand,
    },
    log::warn,
    solana_accounts_db::{
        accounts_db::{
            DEFAULT_ACCOUNTS_SHRINK_OPTIMIZE_TOTAL_SPACE, DEFAULT_ACCOUNTS_SHRINK_RATIO,
        },
        hardened_unpack::MAX_GENESIS_ARCHIVE_UNPACKED_SIZE,
    },
    solana_clap_utils::{
        hidden_unless_forced,
        input_validators::{
            is_keypair, is_keypair_or_ask_keyword, is_parsable, is_pow2, is_pubkey,
            is_pubkey_or_keypair, is_slot, is_url_or_moniker, is_valid_percentage, is_within_range,
            validate_maximum_full_snapshot_archives_to_retain,
            validate_maximum_incremental_snapshot_archives_to_retain,
        },
        keypair::SKIP_SEED_PHRASE_VALIDATION_ARG,
    },
    solana_core::{
        banking_trace::{DirByteLimit, BANKING_TRACE_DIR_DEFAULT_BYTE_LIMIT},
        validator::{BlockProductionMethod, BlockVerificationMethod},
    },
    solana_faucet::faucet::{self, FAUCET_PORT},
    solana_ledger::use_snapshot_archives_at_startup,
    solana_net_utils::{MINIMUM_VALIDATOR_PORT_RANGE_WIDTH, VALIDATOR_PORT_RANGE},
    solana_rayon_threadlimit::get_thread_count,
    solana_rpc::{rpc::MAX_REQUEST_BODY_SIZE, rpc_pubsub_service::PubSubConfig},
    solana_rpc_client_api::request::{DELINQUENT_VALIDATOR_SLOT_DISTANCE, MAX_MULTIPLE_ACCOUNTS},
    solana_runtime::{
        snapshot_bank_utils::{
            DEFAULT_FULL_SNAPSHOT_ARCHIVE_INTERVAL_SLOTS,
            DEFAULT_INCREMENTAL_SNAPSHOT_ARCHIVE_INTERVAL_SLOTS,
        },
        snapshot_utils::{
            SnapshotVersion, DEFAULT_ARCHIVE_COMPRESSION,
            DEFAULT_MAX_FULL_SNAPSHOT_ARCHIVES_TO_RETAIN,
            DEFAULT_MAX_INCREMENTAL_SNAPSHOT_ARCHIVES_TO_RETAIN, SUPPORTED_ARCHIVE_COMPRESSION,
        },
    },
    solana_sdk::{
        clock::Slot, epoch_schedule::MINIMUM_SLOTS_PER_EPOCH, hash::Hash, quic::QUIC_PORT_OFFSET,
        rpc_port,
    },
    solana_send_transaction_service::send_transaction_service::{
        self, MAX_BATCH_SEND_RATE_MS, MAX_TRANSACTION_BATCH_SIZE,
    },
    solana_streamer::quic::DEFAULT_QUIC_ENDPOINTS,
    solana_tpu_client::tpu_client::{DEFAULT_TPU_CONNECTION_POOL_SIZE, DEFAULT_VOTE_USE_QUIC},
    solana_unified_scheduler_pool::DefaultSchedulerPool,
    std::{path::PathBuf, str::FromStr},
};

pub mod thread_args;
use {
    solana_streamer::nonblocking::quic::DEFAULT_MAX_CONNECTIONS_PER_IPADDR_PER_MINUTE,
    thread_args::{thread_args, DefaultThreadArgs},
};

const EXCLUDE_KEY: &str = "account-index-exclude-key";
const INCLUDE_KEY: &str = "account-index-include-key";
// The default minimal snapshot download speed (bytes/second)
const DEFAULT_MIN_SNAPSHOT_DOWNLOAD_SPEED: u64 = 10485760;
// The maximum times of snapshot download abort and retry
const MAX_SNAPSHOT_DOWNLOAD_ABORT: u32 = 5;
// We've observed missed leader slots leading to deadlocks on test validator
// with less than 2 ticks per slot.
const MINIMUM_TICKS_PER_SLOT: u64 = 2;

/// 命令行参数，没有用 clap 的派生宏模式，而是手动构建
pub fn app<'a>(version: &'a str, default_args: &'a DefaultArgs) -> App<'a, 'a> {
    /// ctate 名
    return App::new(crate_name!())
        /// crate 描述
        .about(crate_description!())
        /// crate 版本
        .version(version)
        /// 启用颜色
        .global_setting(AppSettings::ColoredHelp)
        /// 启用子命令前缀匹配
        .global_setting(AppSettings::InferSubcommands)
        /// 不分组标志和选项
        .global_setting(AppSettings::UnifiedHelpMessage)
        /// 子命令不提供版本 -v
        .global_setting(AppSettings::VersionlessSubcommands)
        /// 跳过种子短语的验证。如果你的短语没有使用BIP39官方英语单词表，可以使用这个
        .arg(
            Arg::with_name(SKIP_SEED_PHRASE_VALIDATION_ARG.name)
                .long(SKIP_SEED_PHRASE_VALIDATION_ARG.long)
                .help(SKIP_SEED_PHRASE_VALIDATION_ARG.help),
        )
        /// 节点密钥对
        .arg(
            Arg::with_name("identity")
                .short("i")
                .long("identity")
                .value_name("KEYPAIR")
                .takes_value(true)
                /// 传入一个验证函数，出错就返回一个 string 的原因
                .validator(is_keypair_or_ask_keyword)
                .help("Validator identity keypair"),
        )
        /// 在 Solana 验证器（Validator）的配置中，**`--authorized-voter` 参数** 用于指定一个账户，该账户被授权代表验证器对区块提案（Vote）进行签名和提交。这是验证器运行中的一个关键安全机制，确保投票权限可以独立于验证器的主账户进行管理。以下是详细说明：
        /// ---
        /// ### 1. **`--authorized-voter` 参数的作用**
        /// - 验证器需要参与网络共识，定期对区块的提案进行投票。
        /// - `--authorized-voter` 参数指定了一个 Solana 公钥，代表一个专门的授权账户（Authorized Voter Account）。
        /// - 验证器在运行时，会使用该账户对投票进行签名，并提交到区块链上。
        /// ---
        /// ### 2. **为什么需要 `authorized-voter`**
        /// 1. **分离权限**
        ///    - 验证器节点的主账户（通常是委托资金的账户）与投票账户是分离的。
        ///    - 使用独立的授权账户可以减少安全风险，例如：
        ///    - 如果验证器节点遭到攻击，投票权限不会直接威胁主账户的资金安全。
        /// 2. **账户管理**
        ///    - 在需要更改投票权限时，可以通过更新 `--authorized-voter` 参数指定的新账户，而无需改变验证器主账户或重新初始化整个节点。
        /// 3. **提高灵活性**
        ///    - 多个投票账户可以同时设置为授权账户（通过多次指定 `--authorized-voter` 参数），支持验证器在不同时间段内由不同账户管理投票。
        /// 4. **支持热切换**
        ///    - Solana 的设计允许验证器动态切换到新的授权账户，这对升级或更换账户时非常有用。
        /// ---
        /// ### 3. **使用场景**
        /// - **日常运行**：
        ///   - 验证器节点需要定期对区块进行投票，`--authorized-voter` 参数指定的账户将作为投票的身份。
        /// - **授权切换**：
        ///   - 如果需要替换投票账户，可以在运行验证器时，添加新的 `--authorized-voter` 参数，或者在下一次重启时生效。
        /// - **冷钱包安全**：
        ///   - 主账户可以离线保存，用于委托或管理资金，而投票账户单独在线运行，降低风险。
        .arg(
            Arg::with_name("authorized_voter_keypairs")
                .long("authorized-voter")
                .value_name("KEYPAIR")
                .takes_value(true)
                .validator(is_keypair_or_ask_keyword)
                .requires("vote_account")
                .multiple(true)
                .help(
                    "Include an additional authorized voter keypair. May be specified multiple \
                     times. [default: the --identity keypair]",
                ),
        )
        /// 在 Solana 验证器的配置中，**`vote-account`** 和 **`authorized-voter`** 是两项密切相关的参数。它们共同构成验证器参与共识的关键部分：  
        /// 以下是详细说明两者的关系和用法：
        /// ---
        /// ## 1. **`vote-account` 是什么？**
        /// - **定义**：`vote-account` 是验证器用于记录和提交投票的专用账户，负责存储有关验证器投票历史的数据。
        /// - **用途**：
        ///   - 储存验证器的 **塔式共识数据**（Tower Consensus），防止双签。
        ///   - 接收委托者的质押，并记录这些质押的收益。
        ///   - 提交对区块的提案（Vote）并赚取验证奖励。
        /// ---
        /// ## 2. **`authorized-voter` 是什么？**
        /// - **定义**：`authorized-voter` 是一个授权账户，可以代表投票账户提交区块提案（签名并发送 Vote）。
        /// - **作用**：
        ///   - 通过签名代表投票账户执行操作，例如提交区块提案。
        ///   - 提供了一层额外的安全保障，分离了投票权限与主账户（`validator-identity`）。
        ///   - 支持动态切换，允许多个账户同时作为授权账户。
        /// - **授权关系的设置**：
        ///   - 投票账户需要显式授权哪些账户可以成为 `authorized-voter`。这个操作通常在创建投票账户时完成，或通过 `solana vote-authorize-voter` 命令更新。
        ///   - 示例：
        ///     ```bash
        ///     solana vote-authorize-voter <VOTE_ACCOUNT_PUBKEY> <AUTHORIZED_VOTER_PUBKEY>
        ///     ```
        /// ---
        /// ## 3. **`vote-account` 和 `authorized-voter` 的配合使用**
        /// 1. **运行逻辑**：
        ///    - 验证器启动时，检查 `vote-account` 是否已被授权给指定的 `authorized-voter`。
        ///    - 验证器通过 `authorized-voter` 签名并提交投票，投票记录由 `vote-account` 保存。
        /// ## 4. **实际应用中的建议**
        /// 1. **分离账户角色**：
        ///    - 始终将 `validator-identity`、`vote-account` 和 `authorized-voter` 分开管理。
        ///    - 这样即使 `authorized-voter` 账户泄露，质押资金仍然安全。
        /// 2. **使用热备用账户**：
        ///    - 配置多个 `authorized-voter`，以便在需要时切换授权账户而无需中断验证器运行。
        /// 3. **定期更新授权账户**：
        ///    - 为了增强安全性，定期更换 `authorized-voter` 账户的密钥对。
        /// 4. **监控塔式状态**：
        ///    - 通过 `solana tower` 工具检查投票状态，确保投票正确提交：
        ///      ```bash
        ///      solana tower --vote-account <VOTE_ACCOUNT_PUBKEY>
        ///      ```
        /// ---
        /// ### 总结
        /// - **`vote-account`** 是存储投票数据的核心账户，用于接收委托收益和存储塔式共识数据。
        /// - **`authorized-voter`** 是执行投票操作的授权账户，提供灵活性和安全性。
        .arg(
            Arg::with_name("vote_account")
                .long("vote-account")
                .value_name("ADDRESS")
                .takes_value(true)
                .validator(is_pubkey_or_keypair)
                .requires("identity")
                .help(
                    "Validator vote account public key. If unspecified, voting will be disabled. \
                     The authorized voter for the account must either be the --identity keypair \
                     or set by the --authorized-voter argument",
                ),
        )
        /// 是否在初始化完成后写入一个初始化文件
        .arg(
            Arg::with_name("init_complete_file")
                .long("init-complete-file")
                .value_name("FILE")
                .takes_value(true)
                .help(
                    "Create this file if it doesn't already exist once validator initialization \
                     is complete",
                ),
        )
        /// 账本路径，默认 ./ledger
        .arg(
            Arg::with_name("ledger_path")
                .short("l")
                .long("ledger")
                .value_name("DIR")
                .takes_value(true)
                .required(true)
                .default_value(&default_args.ledger_path)
                .help("Use DIR as ledger location"),
        )
        /// 八卦协议初始连接地址端口
        .arg(
            Arg::with_name("entrypoint")
                .short("n")
                .long("entrypoint")
                .value_name("HOST:PORT")
                .takes_value(true)
                .multiple(true)
                .validator(solana_net_utils::is_host_port)
                .help("Rendezvous with the cluster at this gossip entrypoint"),
        )
        /// 不拉取快照
        .arg(
            Arg::with_name("no_snapshot_fetch")
                .long("no-snapshot-fetch")
                .takes_value(false)
                .help(
                    "Do not attempt to fetch a snapshot from the cluster, start from a local \
                     snapshot if present",
                ),
        )
        /// 不拉取创世块
        .arg(
            Arg::with_name("no_genesis_fetch")
                .long("no-genesis-fetch")
                .takes_value(false)
                .help("Do not fetch genesis from the cluster"),
        )
        /// 不投票
        .arg(
            Arg::with_name("no_voting")
                .long("no-voting")
                .takes_value(false)
                .help("Launch validator without voting"),
        )
        /// 启动时检查投票账号
        .arg(
            Arg::with_name("check_vote_account")
                .long("check-vote-account")
                .takes_value(true)
                .value_name("RPC_URL")
                .requires("entrypoint")
                .conflicts_with_all(&["no_check_vote_account", "no_voting"])
                .help(
                    "Sanity check vote account state at startup. The JSON RPC endpoint at RPC_URL \
                     must expose `--full-rpc-api`",
                ),
        )
        /// 受限修复模式，不会对外发布自己的端口，也不会投票
        .arg(
            Arg::with_name("restricted_repair_only_mode")
                .long("restricted-repair-only-mode")
                .takes_value(false)
                .help(
                    "Do not publish the Gossip, TPU, TVU or Repair Service ports. Doing so causes \
                     the node to operate in a limited capacity that reduces its exposure to the \
                     rest of the cluster. The --no-voting flag is implicit when this flag is \
                     enabled",
                ),
        )
        /// 到达指定时隙时暂停验证器
        .arg(
            Arg::with_name("dev_halt_at_slot")
                .long("dev-halt-at-slot")
                .value_name("SLOT")
                .validator(is_slot)
                .takes_value(true)
                .help("Halt the validator when it reaches the given slot"),
        )
        /// rpc 监听端口
        .arg(
            Arg::with_name("rpc_port")
                .long("rpc-port")
                .value_name("PORT")
                .takes_value(true)
                .validator(port_validator)
                .help("Enable JSON RPC on this port, and the next port for the RPC websocket"),
        )
        /// 启用完整 rpc
        .arg(
            Arg::with_name("full_rpc_api")
                .long("full-rpc-api")
                .conflicts_with("minimal_rpc_api")
                .takes_value(false)
                .help("Expose RPC methods for querying chain state and transaction history"),
        )
        /// 私有化 rpc
        .arg(
            Arg::with_name("private_rpc")
                .long("private-rpc")
                .takes_value(false)
                .help("Do not publish the RPC port for use by others"),
        )
        /// 不检查端口
        .arg(
            Arg::with_name("no_port_check")
                .long("no-port-check")
                .takes_value(false)
                /// 默认不显示在命令帮助里
                .hidden(hidden_unless_forced())
                .help("Do not perform TCP/UDP reachable port checks at start-up"),
        )
        /// rpc提供历史交易查询功能
        .arg(
            Arg::with_name("enable_rpc_transaction_history")
                .long("enable-rpc-transaction-history")
                .takes_value(false)
                .help(
                    "Enable historical transaction info over JSON RPC, including the \
                     'getConfirmedBlock' API. This will cause an increase in disk usage and IOPS",
                ),
        )
        /// 从谷歌大表拉取历史交易
        .arg(
            Arg::with_name("enable_rpc_bigtable_ledger_storage")
                .long("enable-rpc-bigtable-ledger-storage")
                .requires("enable_rpc_transaction_history")
                .takes_value(false)
                .help(
                    "Fetch historical transaction info from a BigTable instance as a fallback to \
                     local ledger data",
                ),
        )
        /// 上传交易到谷歌大表
        .arg(
            Arg::with_name("enable_bigtable_ledger_upload")
                .long("enable-bigtable-ledger-upload")
                .requires("enable_rpc_transaction_history")
                .takes_value(false)
                .help("Upload new confirmed blocks into a BigTable instance"),
        )
        /// 存储扩展交易的元数据
        .arg(
            Arg::with_name("enable_extended_tx_metadata_storage")
                .long("enable-extended-tx-metadata-storage")
                .requires("enable_rpc_transaction_history")
                .takes_value(false)
                .help(
                    "Include CPI inner instructions, logs, and return data in the historical \
                     transaction info stored",
                ),
        )
        /// rpc getMultipleAccounts 的最大账户数，默认 100
        .arg(
            Arg::with_name("rpc_max_multiple_accounts")
                .long("rpc-max-multiple-accounts")
                .value_name("MAX ACCOUNTS")
                .takes_value(true)
                .default_value(&default_args.rpc_max_multiple_accounts)
                .help(
                    "Override the default maximum accounts accepted by the getMultipleAccounts \
                     JSON RPC method",
                ),
        )
        /// health-check-slot-distance 是 Solana 验证器配置中的一个参数，用于控制验证器健康检查的检查间隔。它的作用是帮助 Solana 网络保持健康状态，确保验证器节点的及时同步和有效性。
        /// 1. 定义
        /// health-check-slot-distance：该参数指定了验证器在进行健康检查时，允许其与当前最新区块之间的最大距离（以区块高度 slot 为单位）。如果验证器的区块高度（slot）与最新的区块高度相差超过这个距离，它将被认为存在同步问题，可能被标记为健康不良。
        /// 2. 作用与工作原理
        /// Solana 网络中，验证器必须保持与主链的同步状态，及时处理区块。如果验证器在某一段时间内未能及时验证并提交新区块，它可能会滞后于网络的进展。health-check-slot-distance 就是用来检测并限制验证器的同步状态。
        /// 检查周期：验证器定期与当前最新的区块高度进行比较。通过指定 health-check-slot-distance，Solana 可以允许验证器在健康检查时，最大允许的区块高度差异。
        /// 例如，如果设置的 health-check-slot-distance 为 1000，意味着：
        /// 如果验证器的最新区块高度与 Solana 网络的最新区块高度差异超过 1000，验证器就会被认为存在健康问题。
        /// 网络会尝试通知该验证器，或者标记它为不健康，可能会影响其质押和奖励。
        /// 同步检查：通过此参数，网络能够确保验证器及时处理最新的区块，避免滞后过久造成的分叉风险。
        /// 3. 为什么需要 health-check-slot-distance 参数？
        /// 防止滞后验证器：验证器需要保持与网络同步。如果某个验证器过于滞后，它可能无法有效参与共识或会错过区块。health-check-slot-distance 确保验证器能及时同步和处理新区块。
        /// 增强网络健壮性：通过限制滞后的验证器，Solana 能够保证大部分验证器都能及时更新其区块链状态，减少分叉和无效区块的发生。
        /// 自动化修复机制：如果某个验证器未能在指定的时间内同步到合适的区块高度，它可以被标记为不健康，从而启用相关修复机制，自动重新同步或通知相关管理员进行处理。
        /// 4. 如何配置 health-check-slot-distance？
        /// 验证器启动时，health-check-slot-distance 参数可以通过配置文件或启动命令指定，通常使用如下方式：
        /// bash
        /// 复制代码
        /// solana-validator \
        ///   --health-check-slot-distance <distance_value> \
        ///   --other-options
        /// 其中 <distance_value> 是您希望允许的最大区块高度差。这个值可以根据网络的需求和验证器的性能来调整。
        /// 5. 默认值和建议配置
        /// 默认值：如果未显式配置，Solana 使用默认的 health-check-slot-distance 值。
        /// 推荐配置：这个值的具体设置应基于验证器的同步能力以及网络的状态。例如，验证器性能较差或在不稳定的网络环境下，可能希望增加这个值来避免频繁的健康检查失败。
        /// 常见的建议值为 500 到 2000。过低的值可能会导致较频繁的健康检查失败，而过高的值可能会导致验证器滞后太多，从而影响其共识参与。
        /// 6. 总结
        /// health-check-slot-distance 参数控制验证器在进行健康检查时，允许与最新区块的最大高度差异。
        /// 如果验证器的区块高度与网络的最新区块高度差异超过该值，则验证器可能被视为不健康。
        /// 该参数确保 Solana 网络的验证器保持同步状态，提高网络健壮性。
        /// 默认 128
        .arg(
            Arg::with_name("health_check_slot_distance")
                .long("health-check-slot-distance")
                .value_name("SLOT_DISTANCE")
                .takes_value(true)
                .default_value(&default_args.health_check_slot_distance)
                .help(
                    "Report this validator as healthy if its latest replayed optimistically \
                     confirmed slot is within the specified number of slots from the cluster's \
                     latest optimistically confirmed slot",
                ),
        )
        /// 进行试飞时关闭健康检查
        .arg(
            Arg::with_name("skip_preflight_health_check")
                .long("skip-preflight-health-check")
                .takes_value(false)
                .help(
                    "Skip health check when running a preflight check",
                ),
        )
        /// 空投水龙头监听地址
        .arg(
            Arg::with_name("rpc_faucet_addr")
                .long("rpc-faucet-address")
                .value_name("HOST:PORT")
                .takes_value(true)
                .validator(solana_net_utils::is_host_port)
                .help("Enable the JSON RPC 'requestAirdrop' API with this faucet address."),
        )
        /// 账号存储路径，默认放 <LEDGER>/accounts
        .arg(
            Arg::with_name("account_paths")
                .long("accounts")
                .value_name("PATHS")
                .takes_value(true)
                .multiple(true)
                .help(
                    "Comma separated persistent accounts location. \
                    May be specified multiple times. \
                    [default: <LEDGER>/accounts]",
                ),
        )
        /// 压缩账户集路径
        .arg(
            Arg::with_name("account_shrink_path")
                .long("account-shrink-path")
                .value_name("PATH")
                .takes_value(true)
                .multiple(true)
                .help("Path to accounts shrink path which can hold a compacted account set."),
        )
        /// 账户哈希缓存路径，默认 <LEDGER>/accounts_hash_cache
        .arg(
            Arg::with_name("accounts_hash_cache_path")
                .long("accounts-hash-cache-path")
                .value_name("PATH")
                .takes_value(true)
                .help(
                    "Use PATH as accounts hash cache location \
                     [default: <LEDGER>/accounts_hash_cache]",
                ),
        )
        /// 本地快照路径
        .arg(
            Arg::with_name("snapshots")
                .long("snapshots")
                .value_name("DIR")
                .takes_value(true)
                .help(
                    "Use DIR as the base location for snapshots. \
                     A subdirectory named \"snapshots\" will be created. \
                     [default: --ledger value]",
                 ),
        )
        /// 启动时使用快照存档
        .arg(
            Arg::with_name(use_snapshot_archives_at_startup::cli::NAME)
                .long(use_snapshot_archives_at_startup::cli::LONG_ARG)
                .takes_value(true)
                .possible_values(use_snapshot_archives_at_startup::cli::POSSIBLE_VALUES)
                .default_value(use_snapshot_archives_at_startup::cli::default_value())
                .help(use_snapshot_archives_at_startup::cli::HELP)
                .long_help(use_snapshot_archives_at_startup::cli::LONG_HELP),
        )
        /// 本地全快照路径
        .arg(
            Arg::with_name("full_snapshot_archive_path")
                .long("full-snapshot-archive-path")
                .value_name("DIR")
                .takes_value(true)
                .help(
                    "Use DIR as full snapshot archives location \
                     [default: --snapshots value]",
                 ),
        )
        /// 本地增量快照路径
        .arg(
            Arg::with_name("incremental_snapshot_archive_path")
                .long("incremental-snapshot-archive-path")
                .conflicts_with("no-incremental-snapshots")
                .value_name("DIR")
                .takes_value(true)
                .help(
                    "Use DIR as incremental snapshot archives location \
                     [default: --snapshots value]",
                ),
        )
        /// 存储塔式共识数据
        // ### 塔式共识机制（Tower Consensus）及其在双签防护中的作用
        // Solana 的塔式共识机制（**Tower Consensus**）是其区块链网络中用于快速达成共识的一种创新算法。
        // 它在 Solana 的 Proof-of-History (PoH) 上构建，通过记录和遵守过去的投票记录来确保验证器的行为正确，
        // 并防止双签（Double Signing）等共识问题。
        // 以下是塔式共识的详细原理和防护机制：
        // ---
        // ## 1. **什么是塔式共识？**
        // 塔式共识是一种优化的拜占庭容错（BFT）共识协议，依赖于 Solana 的 PoH 时间排序机制。其核心思想是：
        // - **历史约束**：验证器必须根据其过去的投票记录约束当前的投票行为。
        // - **锁定机制**：通过 "锁定期" 的设计，确保验证器只能对合法的区块链分支进行投票，避免出现双签。
        // ---
        // ## 2. **塔式共识的主要功能**
        // ### 2.1 **历史约束**
        // - 验证器的每次投票都依赖于前一次投票的结果。
        // - 如果某个验证器已经对某个区块高度投票并锁定，则无法回退并对其他分支的区块投票。
        // ### 2.2 **锁定期（Lockout Period）**
        // - 每次投票都会产生一个锁定期，表示验证器对该区块的承诺。
        // - 锁定期以指数形式增加：
        //   - 如果对高度为 `h` 的区块投票，验证器会对该区块锁定一段时间。
        //   - 如果投票给 `h+1` 的区块，锁定期会加倍。
        //   - 例如：
        //     - 高度 10 的锁定期是 2 个区块。
        //     - 高度 11 的锁定期是 4 个区块。
        //     - 高度 12 的锁定期是 8 个区块。
        // - 锁定期确保验证器在达成某个区块的共识后，不会轻易更改分支。
        // ---
        // ## 3. **双签问题及其防护**
        // ### 3.1 **什么是双签？**
        // 双签指验证器在同一区块高度对两个不同的分支投票。这会导致：
        // - 网络分叉。
        // - 破坏区块链的完整性和安全性。
        // - 验证器被惩罚（罚没质押）。
        // ### 3.2 **塔式共识如何防护双签**
        // - **强制遵守锁定规则**：
        //   - 验证器只能对满足其锁定期的分支进行投票，避免在同一高度投票给多个分支。
        //   - 如果尝试双签，网络会检测到其违反锁定规则。
        // - **记录投票历史**：
        //   - 每个验证器的投票历史记录在 `vote-account` 中。
        //   - 如果发现某个投票账户的历史中有冲突，网络会认定该验证器行为恶意。
        // - **罚没机制**：
        //   - 如果验证器被检测到双签，其质押会被罚没（Slashing），严重损害其经济利益。
        // ---
        // ## 4. **塔式共识的运行原理**
        // 以下是塔式共识的简化流程：
        // 1. **验证器生成投票**：
        //    - 验证器根据最新区块状态，选择一个候选区块投票。
        //    - 投票附带 `slot`（区块高度）和 `lockout period`（锁定期）。
        // 2. **验证器提交投票**：
        //    - 验证器通过其 `authorized-voter` 账户签名，并将投票提交到网络中。
        // 3. **验证锁定规则**：
        //    - 验证器只能投票给满足锁定规则的区块：
        //      - 新的投票高度必须高于锁定期内的区块高度。
        //    - 网络会检查投票是否符合历史约束。
        // 4. **更新锁定期**：
        //    - 每次投票都会更新投票账户的锁定期记录。
        //    - 只有在锁定期过后，验证器才能撤销承诺，转而投票给新分支。
        // ---
        // ## 5. **塔式共识的实践建议**
        // 1. **定期监控塔状态**
        //    - 使用 Solana CLI 查看塔式状态，确保投票正常：
        //      ```bash
        //      solana tower --vote-account <VOTE_ACCOUNT_PUBKEY>
        //      ```
        // 2. **避免节点异常**
        //    - 异常重启可能导致塔状态未正确保存，增加双签风险。
        //    - 确保在启动验证器时使用 `--require-tower` 参数：
        //      ```bash
        //      solana-validator \
        //        --require-tower
        //      ```
        // 3. **启用塔式快照（Tower Snapshot）**
        //    - 配置验证器保存塔式快照，防止数据丢失：
        //      ```bash
        //      solana-validator \
        //        --tower-snapshot /path/to/tower-snapshot
        //      ```
        // 4. **定期备份 `vote-account` 数据**
        //    - 备份 `vote-account` 文件，以确保塔记录完整性。
        // 5. **避免多节点共享 `vote-account`**
        //    - 如果多个节点使用相同的投票账户，可能导致双签。
        // ---
        // ### 示例命令：查看投票账户状态
        // 使用 Solana CLI 检查投票账户的塔式状态和锁定期：
        // ```bash
        // solana vote-account <VOTE_ACCOUNT_PUBKEY>
        // ```
        // 输出包括：
        // - 当前的投票高度（Slot）。
        // - 锁定期信息。
        // - 已提交的投票历史。
        // ---
        // ### 总结
        // 塔式共识通过历史约束和锁定期机制确保验证器的行为合规，有效防止双签行为。对于验证器：
        // - 配置正确的投票账户（`vote-account`）和授权账户（`authorized-voter`）。
        // - 使用 `--require-tower` 确保投票历史完整性。
        // - 定期监控和备份投票账户，防止意外数据丢失。
        // 
        // ### 双签惩罚机制及其影响
        // 双签（Double Signing）在区块链网络中是指验证器对同一个区块高度的多个分支进行投票，这会导致网络一致性破坏，因此 Solana 采用了严格的惩罚机制来应对这种恶意行为。
        // 以下是 Solana 如何处理双签及其惩罚机制的详细说明：
        // ---
        // ## 1. **什么是双签？**
        // 双签发生在以下两种情况下：
        // - **同一个验证器在同一区块高度对两个不同的区块分支进行投票**。
        // - **验证器对一个区块进行了多次不同的投票**（例如在不同时间提交不同的投票）。
        // 双签不仅破坏了网络的共识，也使得网络无法从多个验证器中获得一致的状态，导致链的分叉和不确定性。
        // ---
        // ## 2. **为什么 Solana 需要防止双签？**
        // - **确保一致性**：双签会导致验证器无法保证它们在同一区块高度对同一分支的投票一致性。
        // - **保护网络的完整性**：双签会让其他节点和验证器不信任投票者，从而影响整个网络的稳定性。
        // - **提高攻击成本**：通过高额的惩罚，使得攻击双签的成本大大增加，从而增强网络的安全性。
        // ---
        // ## 3. **Solana 的双签惩罚机制**
        // Solana 的双签惩罚机制基于 **质押惩罚（Slashing）**，当验证器被发现进行了双签，其质押会遭受严厉的经济损失。
        // ### 3.1 **质押惩罚**
        // - **质押罚没**：当验证器被检测到双签时，其质押（即用于验证器参与共识的 SOL）将会被罚没。
        // - **罚没的范围**：Solana 会根据网络的共识协议决定具体的罚没比例，但通常是对恶意行为的重罚。罚没的部分通常与验证器的恶意程度直接相关。
        //   举个例子：
        //   - 验证器 A 被发现对同一区块高度的两个分支进行双签，网络将自动惩罚它。Solana 网络可能会选择罚没一定比例的 SOL，可能是 1% 到 50% 的质押。
        //   - 严重的双签行为，可能导致验证器的质押账户完全被扣除。
        // ### 3.2 **惩罚的影响**
        // - **经济损失**：一旦质押被罚没，验证器不仅会失去罚没的 SOL，还可能因此失去参与验证的资格（例如被暂停验证）。
        // - **信誉损失**：双签会让验证器的声誉严重受损。对于委托人来说，这意味着他们的质押可能会被转移，降低验证器未来的委托奖励和资金流入。
        // - **被踢出验证列表**：严重的恶意行为可能会导致验证器永久失去验证资格，并从 Solana 网络的验证者列表中被移除。
        // ---
        // ## 4. **如何检测双签行为**
        // Solana 使用以下方法来检测双签：
        // - **塔式共识和锁定期**：通过塔式共识（Tower Consensus），每次投票会产生一个锁定期，验证器只能对一个有效分支进行投票，违反规则会被系统识别。
        // - **网络同步**：所有投票都公开，且被记录在 Solana 的区块链中。因此，任何验证器的投票行为都会被其他节点同步和验证，双签行为一旦发生，便会立即被检测到。
        // - **网络协议审计**：Solana 节点会定期对投票账户进行审计，确保没有双签行为。任何发现双签的行为都会触发罚没机制。
        // ### 检测双签的命令：
        // Solana 网络会通过其验证器节点在参与共识过程中实时检测是否发生双签行为。可以使用以下命令检查验证器的投票记录：
        // ```bash
        // solana vote-account <VOTE_ACCOUNT_PUBKEY>
        // ```
        // 这个命令会展示验证器的投票状态和历史，帮助检查是否存在冲突或双签行为。
        // ---
        // ## 5. **防止双签的最佳实践**
        // 为了避免发生双签，验证器和委托人应该采取以下措施：
        // ### 5.1 **使用多重授权账户**
        // - **分离角色**：使用多个 `authorized-voter` 账户确保一个账户出问题时，其他账户可以继续正常工作。即使一个授权账户被黑，其他授权账户仍然能正常投票。
        // - **定期切换授权账户**：定期更换授权账户的密钥对，减少密钥泄露的风险。
        // ### 5.2 **确保塔式状态一致性**
        // - **保持塔式状态完整**：使用 `--require-tower` 参数启动验证器，确保验证器正确记录投票历史并防止状态丢失。
        // - **启用塔式快照**：定期备份塔式快照，防止意外的数据丢失。
        //   ```bash
        //   solana-validator --tower-snapshot /path/to/tower-snapshot
        //   ```
        // ### 5.3 **严格监控验证器行为**
        // - **实时监控投票账户**：使用 Solana 的 CLI 或监控工具，确保投票账户没有出现异常或双签行为。
        // - **网络审计工具**：使用 Solana 提供的审计工具定期检查验证器的行为，以防双签行为发生。
        // ---
        // ## 6. **总结**
        // Solana 的双签惩罚机制主要通过 **质押罚没** 的方式来防止恶意行为，确保网络的稳定性和安全性。双签行为会对验证器带来严重的经济损失和声誉损害，甚至可能被移除网络。
        // **防护策略：**
        // - 定期更新投票账户和授权账户。
        // - 确保塔式共识状态的一致性。
        // - 使用合适的备份和监控工具。
        // 通过以上的防护机制，Solana 网络能够确保验证器行为的合规性，并有效防止双签等恶意行为对网络的影响。
        // ---
        .arg(
            Arg::with_name("tower")
                .long("tower")
                .value_name("DIR")
                .takes_value(true)
                .help("Use DIR as file tower storage location [default: --ledger value]"),
        )
        /// 塔式共识存储方案，默认文件
        .arg(
            Arg::with_name("tower_storage")
                .long("tower-storage")
                .possible_values(&["file", "etcd"])
                .default_value(&default_args.tower_storage)
                .takes_value(true)
                .help("Where to store the tower"),
        )
        /// 如果用 etcd，则监听的端口号
        .arg(
            Arg::with_name("etcd_endpoint")
                .long("etcd-endpoint")
                .required_if("tower_storage", "etcd")
                .value_name("HOST:PORT")
                .takes_value(true)
                .multiple(true)
                .validator(solana_net_utils::is_host_port)
                .help("etcd gRPC endpoint to connect with"),
        )
        /// etcd 域名
        .arg(
            Arg::with_name("etcd_domain_name")
                .long("etcd-domain-name")
                .required_if("tower_storage", "etcd")
                .value_name("DOMAIN")
                .default_value(&default_args.etcd_domain_name)
                .takes_value(true)
                .help("domain name against which to verify the etcd server’s TLS certificate"),
        )
        /// etcd 证书文件
        .arg(
            Arg::with_name("etcd_cacert_file")
                .long("etcd-cacert-file")
                .required_if("tower_storage", "etcd")
                .value_name("FILE")
                .takes_value(true)
                .help("verify the TLS certificate of the etcd endpoint using this CA bundle"),
        )
        /// etcd key文件
        .arg(
            Arg::with_name("etcd_key_file")
                .long("etcd-key-file")
                .required_if("tower_storage", "etcd")
                .value_name("FILE")
                .takes_value(true)
                .help("TLS key file to use when establishing a connection to the etcd endpoint"),
        )
        /// etcd 证书文件
        .arg(
            Arg::with_name("etcd_cert_file")
                .long("etcd-cert-file")
                .required_if("tower_storage", "etcd")
                .value_name("FILE")
                .takes_value(true)
                .help("TLS certificate to use when establishing a connection to the etcd endpoint"),
        )
        /// 八卦协议监听端口
        .arg(
            Arg::with_name("gossip_port")
                .long("gossip-port")
                .value_name("PORT")
                .takes_value(true)
                .help("Gossip port number for the validator"),
        )
        /// 八卦协议监听地址
        .arg(
            Arg::with_name("gossip_host")
                .long("gossip-host")
                .value_name("HOST")
                .takes_value(true)
                .validator(solana_net_utils::is_host)
                .help(
                    "Gossip DNS name or IP address for the validator to advertise in gossip \
                     [default: ask --entrypoint, or 127.0.0.1 when --entrypoint is not provided]",
                ),
        )
        /// tpu监听地址
        .arg(
            Arg::with_name("public_tpu_addr")
                .long("public-tpu-address")
                .alias("tpu-host-addr")
                .value_name("HOST:PORT")
                .takes_value(true)
                .validator(solana_net_utils::is_host_port)
                .help(
                    "Specify TPU address to advertise in gossip \
                     [default: ask --entrypoint or localhost when --entrypoint is not provided]",
                ),
        )
        /// tpu转发监听地址
        .arg(
            Arg::with_name("public_tpu_forwards_addr")
                .long("public-tpu-forwards-address")
                .value_name("HOST:PORT")
                .takes_value(true)
                .validator(solana_net_utils::is_host_port)
                .help(
                    "Specify TPU Forwards address to advertise in gossip [default: ask \
                     --entrypoint or localhostwhen --entrypoint is not provided]",
                ),
        )
        /// rpc 监听地址
        .arg(
            Arg::with_name("public_rpc_addr")
                .long("public-rpc-address")
                .value_name("HOST:PORT")
                .takes_value(true)
                .conflicts_with("private_rpc")
                .validator(solana_net_utils::is_host_port)
                .help(
                    "RPC address for the validator to advertise publicly in gossip. Useful for \
                     validators running behind a load balancer or proxy [default: use \
                     --rpc-bind-address / --rpc-port]",
                ),
        )
        /// 动态端口范围
        .arg(
            Arg::with_name("dynamic_port_range")
                .long("dynamic-port-range")
                .value_name("MIN_PORT-MAX_PORT")
                .takes_value(true)
                .default_value(&default_args.dynamic_port_range)
                .validator(port_range_validator)
                .help("Range to use for dynamically assigned ports"),
        )
        /// 最大本地快照有效年龄
        .arg(
            Arg::with_name("maximum_local_snapshot_age")
                .long("maximum-local-snapshot-age")
                .value_name("NUMBER_OF_SLOTS")
                .takes_value(true)
                .default_value(&default_args.maximum_local_snapshot_age)
                .help(
                    "Reuse a local snapshot if it's less than this many slots behind the highest \
                     snapshot available for download from other validators",
                ),
        )
        /// 不使用增量快照
        .arg(
            Arg::with_name("no_incremental_snapshots")
                .long("no-incremental-snapshots")
                .takes_value(false)
                .help("Disable incremental snapshots")
        )
        /// 增量快照归档间隔时隙
        // ### `snapshot-interval-slots` 参数
        // `snapshot-interval-slots` 是 Solana 验证器配置中的一个关键参数，
        // 用于控制验证器生成快照（Snapshot）的频率。快照是 Solana 用于快速恢复网络状态的核心机制之一，
        // 它记录了某个特定区块高度（slot）的链状态。
        // ---
        // ### 1. **定义**
        // - **`snapshot-interval-slots`**：指定验证器在多少个区块（slots）之后生成一次快照。
        // 快照是网络的重要优化工具，可帮助新节点快速同步到当前状态，而无需从创世块（Genesis Block）开始重放所有交易。
        // ---
        // ### 2. **快照的作用**
        // - **快速恢复**：快照存储了完整的区块链状态，包括账户余额、交易记录等。验证器或新节点可以使用快照快速同步，而无需重放所有交易。
        // - **减少同步时间**：对于新加入的节点，直接加载最新的快照，比从头开始重建状态效率高得多。
        // - **降低存储负担**：通过定期生成快照并删除旧数据，验证器可以节省存储空间。
        // ---
        // ### 3. **参数的作用和机制**
        // #### 3.1 **控制生成频率**
        // - 通过设置 `snapshot-interval-slots`，您可以控制验证器多长时间生成一次快照。
        //   - 如果值较小，快照会更频繁地生成，便于其他节点快速获取最新状态。
        //   - 如果值较大，快照生成会减少，从而降低磁盘和 CPU 资源的占用，但可能会导致新节点同步时间变长。
        // #### 3.2 **结合区块高度**
        // - 快照生成总是与区块高度（slot）对齐：
        //   - 假如设置 `snapshot-interval-slots` 为 1000，验证器将在每隔 1000 个区块高度（如 1000、2000、3000）生成一个快照。
        //   - 这些快照被标记为 "Full Snapshot"，表示它们可以用来恢复完整状态。
        // ---
        // ### 4. **如何配置 `snapshot-interval-slots`？**
        // #### 4.1 配置命令
        // 您可以通过以下命令启动验证器并指定快照间隔：
        // ```bash
        // solana-validator \
        //   --snapshot-interval-slots <interval_value> \
        //   --other-options
        // ```
        // - `<interval_value>`：为您希望的快照生成间隔（单位是 slot）。常见值范围为 500 到 5000。
        // #### 4.2 示例
        // 如果设置 `snapshot-interval-slots=1000`：
        // - 验证器将在 slot 高度 1000、2000、3000……生成快照。
        // - 每个快照包含链的完整状态，可以被其他节点加载以快速同步。
        // ---
        // ### 5. **推荐设置**
        // 1. **网络负载较低或节点性能较差**：
        //    - 设置较高的值，例如 2000-5000，减少快照生成的频率，从而降低磁盘和 CPU 的占用。
        // 2. **性能强大的节点或高频繁快照需求**：
        //    - 设置较低的值，例如 500-1000，确保生成更多快照，便于其他节点快速同步。
        // 3. **主网配置参考**：
        //    - Solana 主网一般建议设置为 500-1000 之间，以兼顾快照的频率和资源消耗。
        // ---
        // ### 6. **快照相关的其他参数**
        // #### 6.1 **Incremental Snapshots（增量快照）**
        // - 增量快照只记录从上次完整快照以来的变化，而不是整个链的状态。
        // - 配置参数：
        //   - **`--incremental-snapshot-interval-slots`**：设置生成增量快照的间隔。
        //     ```bash
        //     --incremental-snapshot-interval-slots <interval_value>
        //     ```
        // #### 6.2 **保存快照数量**
        // - 为了节省磁盘空间，您可以限制保存的快照数量：
        //   - **`--maximum-snapshots-to-retain`**：指定保留的快照文件数量。
        //     ```bash
        //     --maximum-snapshots-to-retain 2
        //     ```
        // ---
        // ### 7. **总结**
        // - **`snapshot-interval-slots`** 决定了验证器生成完整快照的频率，是影响同步速度和资源使用的重要参数。
        // - 推荐根据节点性能和网络需求进行调整：
        //   - **高性能节点**：设置较低的间隔（如 500-1000）。
        //   - **资源有限节点**：设置较高的间隔（如 2000-5000）。
        // - 配合增量快照和快照保留参数，可以进一步优化磁盘和网络性能。
        .arg(
            Arg::with_name("snapshot_interval_slots")
                .long("snapshot-interval-slots")
                .alias("incremental-snapshot-interval-slots")
                .value_name("NUMBER")
                .takes_value(true)
                .default_value(&default_args.incremental_snapshot_archive_interval_slots)
                .help("Number of slots between generating snapshots")
                .long_help(
                    "Number of slots between generating snapshots. \
                     If incremental snapshots are enabled, this sets the incremental snapshot interval. \
                     If incremental snapshots are disabled, this sets the full snapshot interval. \
                     Setting this to 0 disables all snapshots.",
                ),
        )
        /// 全快照归档间隔时隙
        .arg(
            Arg::with_name("full_snapshot_interval_slots")
                .long("full-snapshot-interval-slots")
                .value_name("NUMBER")
                .takes_value(true)
                .default_value(&default_args.full_snapshot_archive_interval_slots)
                .help("Number of slots between generating full snapshots")
                .long_help(
                    "Number of slots between generating full snapshots. Must be a multiple of the \
                     incremental snapshot interval. Only used when incremental snapshots are enabled.",
                ),
        )
        /// 最大全快照保留
        .arg(
            Arg::with_name("maximum_full_snapshots_to_retain")
                .long("maximum-full-snapshots-to-retain")
                .alias("maximum-snapshots-to-retain")
                .value_name("NUMBER")
                .takes_value(true)
                .default_value(&default_args.maximum_full_snapshot_archives_to_retain)
                .validator(validate_maximum_full_snapshot_archives_to_retain)
                .help(
                    "The maximum number of full snapshot archives to hold on to when purging \
                     older snapshots.",
                ),
        )
        /// 最大增量快照保留
        .arg(
            Arg::with_name("maximum_incremental_snapshots_to_retain")
                .long("maximum-incremental-snapshots-to-retain")
                .value_name("NUMBER")
                .takes_value(true)
                .default_value(&default_args.maximum_incremental_snapshot_archives_to_retain)
                .validator(validate_maximum_incremental_snapshot_archives_to_retain)
                .help(
                    "The maximum number of incremental snapshot archives to hold on to when \
                     purging older snapshots.",
                ),
        )
        /// 用于调整快照打包（snapshot packaging）进程的优先级，帮助优化验证器的整体性能。
        /// 这个参数通常用于限制快照打包过程对系统资源（尤其是 CPU）的占用，以防止影响验证器的核心任务（如共识和交易处理）。
        .arg(
            Arg::with_name("snapshot_packager_niceness_adj")
                .long("snapshot-packager-niceness-adjustment")
                .value_name("ADJUSTMENT")
                .takes_value(true)
                .validator(solana_perf::thread::is_niceness_adjustment_valid)
                .default_value(&default_args.snapshot_packager_niceness_adjustment)
                .help(
                    "Add this value to niceness of snapshot packager thread. Negative value \
                     increases priority, positive value decreases priority.",
                ),
        )
        /// 最小快照下载速度，如果低于这个速度，就换个节点拉取
        .arg(
            Arg::with_name("minimal_snapshot_download_speed")
                .long("minimal-snapshot-download-speed")
                .value_name("MINIMAL_SNAPSHOT_DOWNLOAD_SPEED")
                .takes_value(true)
                .default_value(&default_args.min_snapshot_download_speed)
                .help(
                    "The minimal speed of snapshot downloads measured in bytes/second. If the \
                     initial download speed falls below this threshold, the system will retry the \
                     download against a different rpc node.",
                ),
        )
        /// 最大快照下载中断次数
        .arg(
            Arg::with_name("maximum_snapshot_download_abort")
                .long("maximum-snapshot-download-abort")
                .value_name("MAXIMUM_SNAPSHOT_DOWNLOAD_ABORT")
                .takes_value(true)
                .default_value(&default_args.max_snapshot_download_abort)
                .help(
                    "The maximum number of times to abort and retry when encountering a slow \
                     snapshot download.",
                ),
        )
        /// 打印八卦连接debug信息的时间间隔，默认 120000 毫秒
        .arg(
            Arg::with_name("contact_debug_interval")
                .long("contact-debug-interval")
                .value_name("CONTACT_DEBUG_INTERVAL")
                .takes_value(true)
                .default_value(&default_args.contact_debug_interval)
                .help("Milliseconds between printing contact debug from gossip."),
        )
        /// 不测试 poh 速度
        .arg(
            Arg::with_name("no_poh_speed_test")
                .long("no-poh-speed-test")
                .hidden(hidden_unless_forced())
                .help("Skip the check for PoH speed."),
        )
        /// 不测试系统网络限制
        .arg(
            Arg::with_name("no_os_network_limits_test")
                .hidden(hidden_unless_forced())
                .long("no-os-network-limits-test")
                .help("Skip checks for OS network limits."),
        )
        /// 不汇报系统内存状态
        .arg(
            Arg::with_name("no_os_memory_stats_reporting")
                .long("no-os-memory-stats-reporting")
                .hidden(hidden_unless_forced())
                .help("Disable reporting of OS memory statistics."),
        )
        /// 不汇报系统网络状态
        .arg(
            Arg::with_name("no_os_network_stats_reporting")
                .long("no-os-network-stats-reporting")
                .hidden(hidden_unless_forced())
                .help("Disable reporting of OS network statistics."),
        )
        /// 不汇报系统cpu状态
        .arg(
            Arg::with_name("no_os_cpu_stats_reporting")
                .long("no-os-cpu-stats-reporting")
                .hidden(hidden_unless_forced())
                .help("Disable reporting of OS CPU statistics."),
        )
        /// 不汇报系统硬盘状态
        .arg(
            Arg::with_name("no_os_disk_stats_reporting")
                .long("no-os-disk-stats-reporting")
                .hidden(hidden_unless_forced())
                .help("Disable reporting of OS disk statistics."),
        )
        /// 快照版本
        .arg(
            Arg::with_name("snapshot_version")
                .long("snapshot-version")
                .value_name("SNAPSHOT_VERSION")
                .validator(is_parsable::<SnapshotVersion>)
                .takes_value(true)
                .default_value(default_args.snapshot_version.into())
                .help("Output snapshot version"),
        )
        /// 限制账本大小，最多存多少个消息分片
        .arg(
            Arg::with_name("limit_ledger_size")
                .long("limit-ledger-size")
                .value_name("SHRED_COUNT")
                .takes_value(true)
                .min_values(0)
                .max_values(1)
                /* .default_value() intentionally not used here! */
                .help("Keep this amount of shreds in root slots."),
        )
        /// rocksdb 压缩存储消息分片的方式
        .arg(
            Arg::with_name("rocksdb_shred_compaction")
                .long("rocksdb-shred-compaction")
                .value_name("ROCKSDB_COMPACTION_STYLE")
                .takes_value(true)
                .possible_values(&["level"])
                .default_value(&default_args.rocksdb_shred_compaction)
                .help(
                    "Controls how RocksDB compacts shreds. *WARNING*: You will lose your \
                     Blockstore data when you switch between options. Possible values are: \
                     'level': stores shreds using RocksDB's default (level) compaction.",
                ),
        )
        /// rocksdb 压缩存储消息分片的压缩算法
        .arg(
            Arg::with_name("rocksdb_ledger_compression")
                .hidden(hidden_unless_forced())
                .long("rocksdb-ledger-compression")
                .value_name("COMPRESSION_TYPE")
                .takes_value(true)
                .possible_values(&["none", "lz4", "snappy", "zlib"])
                .default_value(&default_args.rocksdb_ledger_compression)
                .help(
                    "The compression algorithm that is used to compress transaction status data. \
                     Turning on compression can save ~10% of the ledger size.",
                ),
        )
        /// 收集 rocksdb 读写速度的样本频率
        .arg(
            Arg::with_name("rocksdb_perf_sample_interval")
                .hidden(hidden_unless_forced())
                .long("rocksdb-perf-sample-interval")
                .value_name("ROCKS_PERF_SAMPLE_INTERVAL")
                .takes_value(true)
                .validator(is_parsable::<usize>)
                .default_value(&default_args.rocksdb_perf_sample_interval)
                .help(
                    "Controls how often RocksDB read/write performance samples are collected. \
                     Perf samples are collected in 1 / ROCKS_PERF_SAMPLE_INTERVAL sampling rate.",
                ),
        )
        /// 跳过启动时账本验证
        .arg(
            Arg::with_name("skip_startup_ledger_verification")
                .long("skip-startup-ledger-verification")
                .takes_value(false)
                .help("Skip ledger verification at validator bootup."),
        )
        /// 使用 cuda
        .arg(
            Arg::with_name("cuda")
                .long("cuda")
                .takes_value(false)
                .help("Use CUDA"),
        )
        /// 必须有保存的塔式共识状态
        .arg(
            clap::Arg::with_name("require_tower")
                .long("require-tower")
                .takes_value(false)
                .help("Refuse to start if saved tower state is not found"),
        )
        /// 预期的创世区块哈希
        .arg(
            Arg::with_name("expected_genesis_hash")
                .long("expected-genesis-hash")
                .value_name("HASH")
                .takes_value(true)
                .validator(hash_validator)
                .help("Require the genesis have this hash"),
        )
        /// 用于指定验证器在待网络达到超级多数时应该匹配的区块链状态的哈希值（bank hash）。
        /// 和wait-for-supermajority一起用
        .arg(
            Arg::with_name("expected_bank_hash")
                .long("expected-bank-hash")
                .value_name("HASH")
                .takes_value(true)
                .validator(hash_validator)
                .help("When wait-for-supermajority <x>, require the bank at <x> to have this hash"),
        )
        /// 预期的消息分片版本
        .arg(
            Arg::with_name("expected_shred_version")
                .long("expected-shred-version")
                .value_name("VERSION")
                .takes_value(true)
                .validator(is_parsable::<u16>)
                .help("Require the shred version be this value"),
        )
        /// 日志文件位置
        .arg(
            Arg::with_name("logfile")
                .short("o")
                .long("log")
                .value_name("FILE")
                .takes_value(true)
                .help(
                    "Redirect logging to the specified file, '-' for standard error. Sending the \
                     SIGUSR1 signal to the validator process will cause it to re-open the log file",
                ),
        )
        /// 用于在特定的网络升级或恢复过程中，确保验证器节点等待网络达到超级多数（Supermajority）状态后才开始参与运行。
        /// 这一参数的主要目的是确保网络共识的完整性和安全性。
        // 超级多数：指在网络中，超过 66% 的质押投票权已经通过指定的区块高度。
        // 质押投票权的计算：
        // Solana 根据验证器的质押总量（包括自质押和委托的质押）来确定其投票权。
        // 当超过 66% 的投票权完成了指定的区块高度，网络就达到了超级多数状态。
        .arg(
            Arg::with_name("wait_for_supermajority")
                .long("wait-for-supermajority")
                .requires("expected_bank_hash")
                .requires("expected_shred_version")
                .value_name("SLOT")
                .validator(is_slot)
                .help(
                    "After processing the ledger and the next slot is SLOT, wait until a \
                     supermajority of stake is visible on gossip before starting PoH",
                ),
        )
        /// 用于控制验证器是否必须在开始生成新区块（担任 Leader）之前提交至少一次有效的投票（Vote）。
        .arg(
            Arg::with_name("no_wait_for_vote_to_start_leader")
                .hidden(hidden_unless_forced())
                .long("no-wait-for-vote-to-start-leader")
                .help(
                    "If the validator starts up with no ledger, it will wait to start block \
                     production until it sees a vote land in a rooted slot. This prevents \
                     double signing. Turn off to risk double signing a block.",
                ),
        )
        /// 在某个时隙上硬分叉
        .arg(
            Arg::with_name("hard_forks")
                .long("hard-fork")
                .value_name("SLOT")
                .validator(is_slot)
                .multiple(true)
                .takes_value(true)
                .help("Add a hard fork at this slot"),
        )
        /// 验证器在启动时会检查它从哪些节点接收数据，只允许与 --known-validator 中的节点进行数据交换。
        .arg(
            Arg::with_name("known_validators")
                .alias("trusted-validator")
                .long("known-validator")
                .validator(is_pubkey)
                .value_name("VALIDATOR IDENTITY")
                .multiple(true)
                .takes_value(true)
                .help(
                    "A snapshot hash must be published in gossip by this validator to be \
                     accepted. May be specified multiple times. If unspecified any snapshot hash \
                     will be accepted",
                ),
        )
        /// 处理这个key的事务时打印到日志里
        .arg(
            Arg::with_name("debug_key")
                .long("debug-key")
                .validator(is_pubkey)
                .value_name("ADDRESS")
                .multiple(true)
                .takes_value(true)
                .help("Log when transactions are processed which reference a given key."),
        )
        /// 这一参数通过强制验证器只信任预定义的 RPC 服务端点，增强验证器与网络交互的安全性。
        .arg(
            Arg::with_name("only_known_rpc")
                .alias("no-untrusted-rpc")
                .long("only-known-rpc")
                .takes_value(false)
                .requires("known_validators")
                .help("Use the RPC service of known validators only"),
        )
        /// 用于指定一个或多个可信的验证器节点作为修复数据的来源。当本地验证器节点的数据损坏或不完整时，
        /// 该参数允许从指定的验证器节点获取正确的数据进行修复。
        /// 不配置就不限制
        .arg(
            Arg::with_name("repair_validators")
                .long("repair-validator")
                .validator(is_pubkey)
                .value_name("VALIDATOR IDENTITY")
                .multiple(true)
                .takes_value(true)
                .help(
                    "A list of validators to request repairs from. If specified, repair will not \
                     request from validators outside this set [default: all validators]",
                ),
        )
        /// 相比于repair-validator，这个只是指定的节点优先级提高
        .arg(
            Arg::with_name("repair_whitelist")
                .hidden(hidden_unless_forced())
                .long("repair-whitelist")
                .validator(is_pubkey)
                .value_name("VALIDATOR IDENTITY")
                .multiple(true)
                .takes_value(true)
                .help(
                    "A list of validators to prioritize repairs from. If specified, repair \
                     requests from validators in the list will be prioritized over requests from \
                     other validators. [default: all validators]",
                ),
        )
        /// 八卦节点，不写就是都行
        .arg(
            Arg::with_name("gossip_validators")
                .long("gossip-validator")
                .validator(is_pubkey)
                .value_name("VALIDATOR IDENTITY")
                .multiple(true)
                .takes_value(true)
                .help(
                    "A list of validators to gossip with. If specified, gossip will not \
                     push/pull from from validators outside this set. [default: all validators]",
                ),
        )
        /// 报文合并在TPU接收器上等待的毫秒数
        .arg(
            Arg::with_name("tpu_coalesce_ms")
                .long("tpu-coalesce-ms")
                .value_name("MILLISECS")
                .takes_value(true)
                .validator(is_parsable::<u64>)
                .help("Milliseconds to wait in the TPU receiver for packet coalescing."),
        )
        /// 使用QUIC发送tpu
        .arg(
            Arg::with_name("tpu_use_quic")
                .long("tpu-use-quic")
                .takes_value(false)
                .hidden(hidden_unless_forced())
                .conflicts_with("tpu_disable_quic")
                .help("Use QUIC to send transactions."),
        )
        /// 禁用QUIC发送tpu
        .arg(
            Arg::with_name("tpu_disable_quic")
                .long("tpu-disable-quic")
                .takes_value(false)
                .help("Do not use QUIC to send transactions."),
        )
        /// 使用udp发送tpu
        // ### QUIC 和 UDP 的关系与区别
        // **QUIC（Quick UDP Internet Connections）** 是一种基于 **UDP（User Datagram Protocol）** 的现代化网络传输协议，专注于快速、安全的网络连接。它结合了 UDP 的轻量特性和许多现代化改进，是一种为互联网优化的传输协议。
        // 以下是 QUIC 和 UDP 的详细对比与解释：
        // ---
        // ### 1. **什么是 UDP？**
        // #### 定义
        // - **UDP** 是一种面向数据报的简单传输层协议，位于 IP 层之上。
        // - 它提供快速、不可靠的传输，不进行连接建立，也不保证数据的顺序或完整性。
        // #### 特性
        // 1. **无连接**：
        //    - 发送方和接收方无需建立连接，数据直接发送到目标地址。
        // 2. **轻量**：
        //    - 没有握手机制和状态管理，开销极低。
        // 3. **不可靠传输**：
        //    - 不保证数据包的顺序、不提供丢包重传。
        // 4. **典型用途**：
        //    - 实时通信（如视频、语音通话）。
        //    - DNS 查询、游戏等对低延迟需求高的场景。
        // ---
        // ### 2. **什么是 QUIC？**
        // #### 定义
        // - **QUIC** 是一种基于 UDP 的高级传输协议，由 Google 开发并成为 IETF 标准（RFC 9000）。
        // - 目标是为互联网通信提供更高效、更安全、更低延迟的传输。
        // #### 特性
        // 1. **基于 UDP**：
        //    - QUIC 使用 UDP 作为底层传输协议，继承其轻量和无连接的特性。
        //    - 通过在应用层实现连接管理和可靠性，解决了 UDP 的不可靠问题。
        // 2. **内置加密**：
        //    - 使用 TLS 1.3 加密连接，提供类似 HTTPS 的安全性。
        // 3. **多路复用**：
        //    - 单个 QUIC 连接可以承载多个数据流，避免了 TCP 中的**队头阻塞**问题。
        // 4. **快速连接建立**：
        //    - 通过 0-RTT（Zero Round Trip Time）握手，大幅降低连接建立的延迟。
        // 5. **拥塞控制和丢包恢复**：
        //    - QUIC 自带拥塞控制算法（如 Cubic、BBR），并通过帧重传机制实现可靠性。
        // 6. **典型用途**：
        //    - HTTP/3 是基于 QUIC 的最新 HTTP 协议版本。
        //    - 视频流服务（如 YouTube）、游戏、实时通信等。
        // ---
        // ### 3. **QUIC 和 UDP 的关系**
        // - **基础协议**：QUIC 构建在 UDP 之上，利用其无连接和快速传输的特性。
        // - **扩展功能**：QUIC 在应用层实现了可靠性、加密、多路复用等特性，弥补了 UDP 的不足。
        // ---
        // ### 4. **QUIC 和 UDP 的对比**
        // | 特性                 | UDP                         | QUIC                                   |
        // |----------------------|-----------------------------|----------------------------------------|
        // | **传输层**           | 传输层协议                  | 应用层协议（基于 UDP）                  |
        // | **连接管理**         | 无连接                      | 支持连接管理（虚拟连接）               |
        // | **数据可靠性**       | 不保证                      | 提供数据重传和顺序管理                 |
        // | **安全性**           | 无内置加密                  | 内置 TLS 1.3 加密                      |
        // | **多路复用**         | 不支持                      | 支持                                   |
        // | **连接建立延迟**     | 无需握手                    | 支持 0-RTT，几乎无延迟                 |
        // | **用途**             | 实时通信、DNS、游戏         | HTTP/3、视频流、低延迟的安全通信       |
        // ---
        // ### 5. **应用场景**
        // #### 5.1 **UDP 的应用场景**
        // - **实时通信**：如视频会议（Zoom）和语音通话（VoIP）。
        // - **在线游戏**：如 FPS 游戏，需要低延迟和快速数据传输。
        // - **DNS 查询**：对延迟敏感但对丢包容忍的应用。
        // #### 5.2 **QUIC 的应用场景**
        // - **HTTP/3**：现代化的网页传输协议，广泛应用于浏览器和网站。
        // - **流媒体**：如 YouTube 和 Netflix，提供更流畅的视频播放。
        // - **在线游戏**：需要低延迟和高安全性的游戏应用。
        // - **实时通信**：高效、可靠的语音和视频传输。
        // ---
        // ### 6. **优劣势对比**
        // | **特性**                | **UDP 的优劣**                                         | **QUIC 的优劣**                                         |
        // |-------------------------|-------------------------------------------------------|-------------------------------------------------------|
        // | **性能**                | 高效、低开销，但不保证可靠性                           | 高性能，多路复用，可靠性强，稍高的协议开销            |
        // | **可靠性**              | 不保证，需应用层实现                                   | 内置可靠性，提供数据重传                              |
        // | **安全性**              | 无加密，需结合其他协议（如 DTLS）                      | 内置加密，支持 TLS 1.3                                |
        // | **延迟**                | 极低，无握手                                           | 支持 0-RTT，几乎无连接延迟                            |
        // | **实现复杂度**          | 简单，适合特定场景                                     | 较复杂，需要强大的开发与调试支持                      |
        // ---
        // ### 7. **总结**
        // - **UDP**：
        //   - 优点：简单、快速、低开销，适合需要低延迟但对可靠性要求不高的场景。
        //   - 缺点：不提供可靠性和安全性，需要应用层自行实现。
        // - **QUIC**：
        //   - 优点：结合了 UDP 的快速性和 TCP 的可靠性，同时内置加密和多路复用，是现代互联网通信的优选。
        //   - 缺点：协议复杂度高，相比 UDP 开销稍大。
        // ---
        .arg(
            Arg::with_name("tpu_enable_udp")
                .long("tpu-enable-udp")
                .takes_value(false)
                .help("Enable UDP for receiving/sending transactions."),
        )
        /// 每个地址的tpu连接池大小
        .arg(
            Arg::with_name("tpu_connection_pool_size")
                .long("tpu-connection-pool-size")
                .takes_value(true)
                .default_value(&default_args.tpu_connection_pool_size)
                .validator(is_parsable::<usize>)
                .help("Controls the TPU connection pool size per remote address"),
        )
        /// tpu每分钟每个ip地址的连接数上限
        .arg(
            Arg::with_name("tpu_max_connections_per_ipaddr_per_minute")
                .long("tpu-max-connections-per-ipaddr-per-minute")
                .takes_value(true)
                .default_value(&default_args.tpu_max_connections_per_ipaddr_per_minute)
                .validator(is_parsable::<u32>)
                .hidden(hidden_unless_forced())
                .help("Controls the rate of the clients connections per IpAddr per minute."),
        )
        /// 使用 quic 投票
        .arg(
            Arg::with_name("vote_use_quic")
                .long("vote-use-quic")
                .takes_value(true)
                .default_value(&default_args.vote_use_quic)
                .hidden(hidden_unless_forced())
                .help("Controls if to use QUIC to send votes."),
        )
        /// QUIC 端点数
        .arg(
            Arg::with_name("num_quic_endpoints")
                .long("num-quic-endpoints")
                .takes_value(true)
                .default_value(&default_args.num_quic_endpoints)
                .validator(is_parsable::<usize>)
                .hidden(hidden_unless_forced())
                .help("The number of QUIC endpoints used for TPU and TPU-Forward. It can be increased to \
                       increase network ingest throughput, at the expense of higher CPU and general \
                       validator load."),
        )
        // `--staked-nodes-overrides` 是 Solana 节点配置中的一个参数，用于允许用户为特定的验证节点设置自定义的质押（staking）权重或调整节点的投票行为。这个参数通常用于配置特定节点在验证网络中的行为或权重，以确保特定节点在投票和验证过程中获得不同的优先级或权重。
        // ### 参数作用：
        // `--staked-nodes-overrides` 参数使用户可以通过提供一个包含节点 ID 和对应的质押权重或相关设置的配置文件来调整节点的行为。这允许用户有选择地改变某些节点的投票影响力或质押状态，通常用于测试、优化性能或特定网络设置。
        // ### 参数格式：
        // 该参数的配置格式通常包含节点的公钥（node identity）以及对应的调整设置，通常是一个 JSON 文件。例如，它可能看起来像这样：
        // ```json
        // {
        //   "node1PublicKey": {
        //     "stake": 1000,
        //     "vote": 1
        //   },
        //   "node2PublicKey": {
        //     "stake": 500,
        //     "vote": 0
        //   }
        // }
        // ```
        // 在这个例子中：
        // - `node1PublicKey` 表示某个节点的公钥，并且该节点的质押值（`stake`）是 1000，而投票权重（`vote`）是 1。
        // - `node2PublicKey` 是另一个节点的公钥，它的质押值为 500，而投票权重为 0，意味着这个节点不会被用来投票。
        // ### 用途：
        // 1. **调整节点优先级**：可以调整某些节点的质押权重或投票权重，使其在网络中具有更高或更低的影响力。
        // 2. **优化性能**：在某些情况下，可能希望在节点配置中对某些节点进行特殊调整，以达到性能优化的目的。
        // 3. **定制行为**：允许对特定节点进行更详细的定制化配置，适用于需要特定行为的网络环境或测试场景。
        // ### 示例：
        // 启动节点时使用 `--staked-nodes-overrides` 参数，例如：
        // ```
        // --staked-nodes-overrides /path/to/overrides.json
        // ```
        // 此时，Solana 节点会读取指定的 `overrides.json` 文件，并根据文件中的配置调整节点的质押和投票设置。
        .arg(
            Arg::with_name("staked_nodes_overrides")
                .long("staked-nodes-overrides")
                .value_name("PATH")
                .takes_value(true)
                .help(
                    "Provide path to a yaml file with custom overrides for stakes of specific \
                     identities. Overriding the amount of stake this validator considers as valid \
                     for other peers in network. The stake amount is used for calculating the \
                     number of QUIC streams permitted from the peer and vote packet sender stage. \
                     Format of the file: `staked_map_id: {<pubkey>: <SOL stake amount>}",
                ),
        )
        /// 监听地址
        .arg(
            Arg::with_name("bind_address")
                .long("bind-address")
                .value_name("HOST")
                .takes_value(true)
                .validator(solana_net_utils::is_host)
                .default_value(&default_args.bind_address)
                .help("IP address to bind the validator ports"),
        )
        /// rpc 监听地址
        .arg(
            Arg::with_name("rpc_bind_address")
                .long("rpc-bind-address")
                .value_name("HOST")
                .takes_value(true)
                .validator(solana_net_utils::is_host)
                .help(
                    "IP address to bind the RPC port [default: 127.0.0.1 if --private-rpc is \
                     present, otherwise use --bind-address]",
                ),
        )
        /// rpc 线程数
        .arg(
            Arg::with_name("rpc_threads")
                .long("rpc-threads")
                .value_name("NUMBER")
                .validator(is_parsable::<usize>)
                .takes_value(true)
                .default_value(&default_args.rpc_threads)
                .help("Number of threads to use for servicing RPC requests"),
        )
        /// rpc阻塞线程数，用来扫账户之类的
        .arg(
            Arg::with_name("rpc_blocking_threads")
                .long("rpc-blocking-threads")
                .value_name("NUMBER")
                .validator(is_parsable::<usize>)
                .validator(|value| {
                    value
                        .parse::<u64>()
                        .map_err(|err| format!("error parsing '{value}': {err}"))
                        .and_then(|threads| {
                            if threads > 0 {
                                Ok(())
                            } else {
                                Err("value must be >= 1".to_string())
                            }
                        })
                })
                .takes_value(true)
                .default_value(&default_args.rpc_blocking_threads)
                .help("Number of blocking threads to use for servicing CPU bound RPC requests (eg getMultipleAccounts)"),
        )
        /// rpc进程优先级
        .arg(
            Arg::with_name("rpc_niceness_adj")
                .long("rpc-niceness-adjustment")
                .value_name("ADJUSTMENT")
                .takes_value(true)
                .validator(solana_perf::thread::is_niceness_adjustment_valid)
                .default_value(&default_args.rpc_niceness_adjustment)
                .help(
                    "Add this value to niceness of RPC threads. Negative value increases \
                     priority, positive value decreases priority.",
                ),
        )
        /// rpc 大表超时秒数
        .arg(
            Arg::with_name("rpc_bigtable_timeout")
                .long("rpc-bigtable-timeout")
                .value_name("SECONDS")
                .validator(is_parsable::<u64>)
                .takes_value(true)
                .default_value(&default_args.rpc_bigtable_timeout)
                .help("Number of seconds before timing out RPC requests backed by BigTable"),
        )
        /// rpc大表实例名
        .arg(
            Arg::with_name("rpc_bigtable_instance_name")
                .long("rpc-bigtable-instance-name")
                .takes_value(true)
                .value_name("INSTANCE_NAME")
                .default_value(&default_args.rpc_bigtable_instance_name)
                .help("Name of the Bigtable instance to upload to"),
        )
        /// rpc 大表 id
        .arg(
            Arg::with_name("rpc_bigtable_app_profile_id")
                .long("rpc-bigtable-app-profile-id")
                .takes_value(true)
                .value_name("APP_PROFILE_ID")
                .default_value(&default_args.rpc_bigtable_app_profile_id)
                .help("Bigtable application profile id to use in requests"),
        )
        /// rpc 大表消息最大大小
        .arg(
            Arg::with_name("rpc_bigtable_max_message_size")
                .long("rpc-bigtable-max-message-size")
                .value_name("BYTES")
                .validator(is_parsable::<usize>)
                .takes_value(true)
                .default_value(&default_args.rpc_bigtable_max_message_size)
                .help("Max encoding and decoding message size used in Bigtable Grpc client"),
        )
        /// 订阅类 rpc 的线程数
        .arg(
            Arg::with_name("rpc_pubsub_worker_threads")
                .long("rpc-pubsub-worker-threads")
                .takes_value(true)
                .value_name("NUMBER")
                .validator(is_parsable::<usize>)
                .default_value(&default_args.rpc_pubsub_worker_threads)
                .help("PubSub worker threads"),
        )
        /// rpc 块订阅功能启用
        .arg(
            Arg::with_name("rpc_pubsub_enable_block_subscription")
                .long("rpc-pubsub-enable-block-subscription")
                .requires("enable_rpc_transaction_history")
                .takes_value(false)
                .help("Enable the unstable RPC PubSub `blockSubscribe` subscription"),
        )
        /// rpc 投票订阅功能启用
        .arg(
            Arg::with_name("rpc_pubsub_enable_vote_subscription")
                .long("rpc-pubsub-enable-vote-subscription")
                .takes_value(false)
                .help("Enable the unstable RPC PubSub `voteSubscribe` subscription"),
        )
        /// rpc 订阅的数量上限
        .arg(
            Arg::with_name("rpc_pubsub_max_active_subscriptions")
                .long("rpc-pubsub-max-active-subscriptions")
                .takes_value(true)
                .value_name("NUMBER")
                .validator(is_parsable::<usize>)
                .default_value(&default_args.rpc_pubsub_max_active_subscriptions)
                .help(
                    "The maximum number of active subscriptions that RPC PubSub will accept \
                     across all connections.",
                ),
        )
        /// rpc 最大通知数
        .arg(
            Arg::with_name("rpc_pubsub_queue_capacity_items")
                .long("rpc-pubsub-queue-capacity-items")
                .takes_value(true)
                .value_name("NUMBER")
                .validator(is_parsable::<usize>)
                .default_value(&default_args.rpc_pubsub_queue_capacity_items)
                .help(
                    "The maximum number of notifications that RPC PubSub will store across all \
                     connections.",
                ),
        )
        /// rpc 最大通知字节数
        .arg(
            Arg::with_name("rpc_pubsub_queue_capacity_bytes")
                .long("rpc-pubsub-queue-capacity-bytes")
                .takes_value(true)
                .value_name("BYTES")
                .validator(is_parsable::<usize>)
                .default_value(&default_args.rpc_pubsub_queue_capacity_bytes)
                .help(
                    "The maximum total size of notifications that RPC PubSub will store across \
                     all connections.",
                ),
        )
        /// rpc 通知线程数
        .arg(
            Arg::with_name("rpc_pubsub_notification_threads")
                .long("rpc-pubsub-notification-threads")
                .requires("full_rpc_api")
                .takes_value(true)
                .value_name("NUM_THREADS")
                .validator(is_parsable::<usize>)
                .default_value_if(
                    "full_rpc_api",
                    None,
                    &default_args.rpc_pubsub_notification_threads,
                )
                .help(
                    "The maximum number of threads that RPC PubSub will use for generating \
                     notifications. 0 will disable RPC PubSub notifications",
                ),
        )
        // Solana 的验证器节点（Validator）不仅负责验证交易和区块，还会向网络发送交易，特别是在以下几种情况下：
        // 1. 创建和发送投票交易（Vote Transactions）
        // 验证器节点需要定期提交投票交易，以证明其参与网络共识并为区块链的进度投票。这些投票交易是为了维持验证器节点在共识中的活跃度，确保网络能够继续稳定运行。
        // 2. 提交质押相关交易（Staking Transactions）
        // 验证器节点通常参与质押（staking）过程，向网络提交质押或取消质押的交易。这些交易也需要被发送并确认，以维持网络的健康和验证器的参与度。
        // 3. 发送交易以激活某些功能
        // 在某些情况下，验证器节点可能需要发送交易来激活特定的功能或进行配置更新，这些交易也会作为普通的 Solana 网络交易被处理。
        /// rpc 发送交易重试秒数
        /// 
        // Solana 的验证器节点发送的很多交易，包括投票交易、质押交易等，确实是通过 RPC（Remote Procedure Call，远程过程调用）发送的，而不是直接通过 Gossip 协议。这是由于 RPC 和 Gossip 在 Solana 网络中有不同的作用和优化目标。
        // 为什么交易通过 RPC 而不是 Gossip 发送？
        // 1. Gossip 协议与数据传播
        // Gossip 协议主要用于在网络中传播信息和数据，尤其是为了帮助节点同步状态。它通常用于以下场景：
        // 区块和交易的广播：节点之间通过 Gossip 协议互相传播新区块和交易，以实现分布式共识。
        // 状态同步：节点通过 Gossip 共享关于账户、区块链状态和其他元数据的信息。
        // Gossip 是一个基于 P2P（点对点）协议的机制，适合传播节点间的状态和区块数据，但并不专门为高优先级、持久性的请求设计，如发送交易或查询。Gossip 协议本身并不保证请求的确认或可靠性，因此它不是一个可靠的机制来发送需要保证最终一致性或状态更改的交易。
        // 2. RPC 用于确保交易的可靠性
        // RPC 是用于客户端和服务器之间通信的协议，它能够提供以下功能：
        // 可靠性：RPC 通过请求/响应机制确保了交易的可靠传输，确保交易能够被记录到网络中，并被确认。这对于验证器发送投票交易和质押交易等具有高可靠性要求的操作尤其重要。
        // 交易确认：RPC 请求能够提供反馈，如确认交易是否成功或发生了错误，从而让客户端（例如验证器节点）根据需要进行后续操作。
        // 确保交易处理：RPC 请求会通过 Solana 网络的各个节点进行处理和验证，并返回确认信息。验证器节点需要这些确认来确保它们发送的交易已经被包括在区块链中。
        // 对于验证器来说，发送交易并确保交易最终被网络接受并成功处理，是其核心职责之一，而 RPC 提供了这种可靠的通信方式。
        // 3. 区分不同用途
        // Gossip 协议：用于节点间的状态传播，帮助节点了解网络中的其他节点和区块链的状态信息，特别是有关新区块和交易的信息。
        // RPC：用于节点向其他节点（尤其是外部 RPC 服务）请求高优先级操作，通常是交易提交和查询。RPC 请求通常涉及需要确认和反馈的操作，保证交易能够被准确地传递和记录。
        // 4. 交易确认与状态管理
        // Gossip 协议并不提供请求和响应机制，因此它不适合处理涉及确认、状态变化和错误处理的事务。RPC 协议则适合用于这种场景，因为它能确保交易的最终确认，提供重试机制，并在请求失败时提供错误反馈。
        // 5. 交易类型和使用场景
        // 投票交易：验证器发送投票交易以参与共识，通常通过 RPC 发送，并且这些交易需要被确认为区块的一部分。
        // 质押交易：验证器提交质押交易来参与质押奖励或退出质押，也需要通过 RPC 确认。
        // 总结
        // Gossip 协议：用于网络内的区块传播、节点发现、状态共享等操作，优化数据传播和状态同步。
        // RPC 协议：用于向 Solana 网络提交需要高可靠性、交易确认和反馈的操作，如验证器的投票、质押交易等。
        // 由于 RPC 提供了更高的可靠性、交易确认和反馈机制，它被用于提交验证器的交易（如投票和质押交易），而 Gossip 更适合在节点之间传播数据和共享状态。因此，验证器不会直接通过 Gossip 发送交易，而是使用 RPC 以确保这些交易能够被处理、确认并最终记录在区块链中。
        // Solana 验证器在发送投票交易时，通常会将这些交易发送到自己所运行的 RPC 服务（即自己的节点），而不是直接发送到其他节点。这是因为验证器本身会作为区块链的一部分，参与网络共识，并且它需要向自己的节点提交投票交易来表达对某个区块或状态的验证同意。
        // 具体流程：
        // 验证器投票交易的生成： 验证器节点会根据其自身的共识参与情况生成投票交易。这些投票是用于表示该验证器对于某个区块的认可（即投票证明该区块是有效的）。这些交易包含了验证器节点的签名和选定的区块信息。
        // 发送投票交易到自己的节点： 由于验证器节点已经通过 RPC 连接到网络，它会将投票交易发送到自己的 RPC 服务接口。这个过程通常通过以下方式实现：
        // 验证器节点会在自己的本地 RPC 服务上发送投票交易请求，指示它将某个特定区块添加到链中。
        // 通过 RPC 进行交易提交时，验证器会利用自己的节点 RPC 服务来验证、广播交易并等待确认。
        // 通过 RPC 提交到网络：
        // 通过 RPC 提交的投票交易会被本地验证器节点先处理，然后再通过网络传播到其他节点。
        // 其他节点会通过 Gossip 协议等机制接收到投票交易，并在区块链中进行处理。
        // 确认投票交易： 投票交易是验证器提交的一个重要操作，网络中其他节点需要确认并记录这个交易，确保验证器的投票得到有效记录。如果投票交易没有正确处理，验证器可能会被从共识中排除，失去奖励。
        // 投票交易发送的节点入口点：
        // RPC 服务：验证器节点会通过它自己的 RPC 服务入口点发送投票交易。通常这是验证器本地的 RPC 接口，确保投票交易可以直接提交并得到处理。
        // 外部连接：如果验证器连接到外部服务或其他节点，也可以通过外部 RPC 服务发送投票交易，但通常验证器会优先使用自己的 RPC 服务来发送和确认投票交易。
        .arg(
            Arg::with_name("rpc_send_transaction_retry_ms")
                .long("rpc-send-retry-ms")
                .value_name("MILLISECS")
                .takes_value(true)
                .validator(is_parsable::<u64>)
                .default_value(&default_args.rpc_send_transaction_retry_ms)
                .help("The rate at which transactions sent via rpc service are retried."),
        )
        /// rpc 按批发送交易重试秒数
        .arg(
            Arg::with_name("rpc_send_transaction_batch_ms")
                .long("rpc-send-batch-ms")
                .value_name("MILLISECS")
                .hidden(hidden_unless_forced())
                .takes_value(true)
                .validator(|s| is_within_range(s, 1..=MAX_BATCH_SEND_RATE_MS))
                .default_value(&default_args.rpc_send_transaction_batch_ms)
                .help("The rate at which transactions sent via rpc service are sent in batch."),
        )
        /// 交易要发给几个leader
        .arg(
            Arg::with_name("rpc_send_transaction_leader_forward_count")
                .long("rpc-send-leader-count")
                .value_name("NUMBER")
                .takes_value(true)
                .validator(is_parsable::<u64>)
                .default_value(&default_args.rpc_send_transaction_leader_forward_count)
                .help(
                    "The number of upcoming leaders to which to forward transactions sent via rpc \
                     service.",
                ),
        )
        /// rpc 发送交易最大重试次数
        .arg(
            Arg::with_name("rpc_send_transaction_default_max_retries")
                .long("rpc-send-default-max-retries")
                .value_name("NUMBER")
                .takes_value(true)
                .validator(is_parsable::<usize>)
                .help(
                    "The maximum number of transaction broadcast retries when unspecified by the \
                     request, otherwise retried until expiration.",
                ),
        )
        // rpc-send-default-max-retries 用于普通 RPC 请求的重试控制。
        // rpc-send-service-max-retries 用于与外部 RPC 服务的请求重试控制。
        .arg(
            Arg::with_name("rpc_send_transaction_service_max_retries")
                .long("rpc-send-service-max-retries")
                .value_name("NUMBER")
                .takes_value(true)
                .validator(is_parsable::<usize>)
                .default_value(&default_args.rpc_send_transaction_service_max_retries)
                .help(
                    "The maximum number of transaction broadcast retries, regardless of requested \
                     value.",
                ),
        )
        /// 发送交易的批次大小
        .arg(
            Arg::with_name("rpc_send_transaction_batch_size")
                .long("rpc-send-batch-size")
                .value_name("NUMBER")
                .hidden(hidden_unless_forced())
                .takes_value(true)
                .validator(|s| is_within_range(s, 1..=MAX_TRANSACTION_BATCH_SIZE))
                .default_value(&default_args.rpc_send_transaction_batch_size)
                .help("The size of transactions to be sent in batch."),
        )
        /// 发送交易的重试池最大尺寸
        .arg(
            Arg::with_name("rpc_send_transaction_retry_pool_max_size")
                .long("rpc-send-transaction-retry-pool-max-size")
                .value_name("NUMBER")
                .takes_value(true)
                .validator(is_parsable::<usize>)
                .default_value(&default_args.rpc_send_transaction_retry_pool_max_size)
                .help("The maximum size of transactions retry pool."),
        )
        /// 把交易发给别的对等体的tpu
        .arg(
            Arg::with_name("rpc_send_transaction_tpu_peer")
                .long("rpc-send-transaction-tpu-peer")
                .takes_value(true)
                .number_of_values(1)
                .multiple(true)
                .value_name("HOST:PORT")
                .validator(solana_net_utils::is_host_port)
                .help("Peer(s) to broadcast transactions to instead of the current leader")
        )
        /// 交易也发给当前leader
        .arg(
            Arg::with_name("rpc_send_transaction_also_leader")
                .long("rpc-send-transaction-also-leader")
                .requires("rpc_send_transaction_tpu_peer")
                .help("With `--rpc-send-transaction-tpu-peer HOST:PORT`, also send to the current leader")
        )
        // 扫描和修复根：该参数使 Solana 节点在运行时检查其存储的“根”信息，并修复任何可能存在的错误或不一致。
        // 根信息：在 Solana 网络中，节点维护的是区块链的状态（包括账户和交易历史）。每个区块的根（root）是区块链的基础数据结构的一部分，它确保区块链的有序性和一致性。
        // 确保一致性：当节点的状态或存储出现问题时，使用 rpc-scan-and-fix-roots 可以帮助修复与根相关的任何不一致问题。这是特别在节点状态出现异常时非常重要的功能。
        .arg(
            Arg::with_name("rpc_scan_and_fix_roots")
                .long("rpc-scan-and-fix-roots")
                .takes_value(false)
                .requires("enable_rpc_transaction_history")
                .help("Verifies blockstore roots on boot and fixes any gaps"),
        )
        /// 能接受的rpc请求最大请求体大小
        .arg(
            Arg::with_name("rpc_max_request_body_size")
                .long("rpc-max-request-body-size")
                .value_name("BYTES")
                .takes_value(true)
                .validator(is_parsable::<usize>)
                .default_value(&default_args.rpc_max_request_body_size)
                .help("The maximum request body size accepted by rpc service"),
        )
        /// geyser 插件配置文件路径
        .arg(
            Arg::with_name("geyser_plugin_config")
                .long("geyser-plugin-config")
                .alias("accountsdb-plugin-config")
                .value_name("FILE")
                .takes_value(true)
                .multiple(true)
                .help("Specify the configuration file for the Geyser plugin."),
        )
        /// 没有配置文件时也使用geyser
        .arg(
            Arg::with_name("geyser_plugin_always_enabled")
                .long("geyser-plugin-always-enabled")
                .value_name("BOOLEAN")
                .takes_value(false)
                .help("Еnable Geyser interface even if no Geyser configs are specified."),
        )
        /// 快照归档格式
        .arg(
            Arg::with_name("snapshot_archive_format")
                .long("snapshot-archive-format")
                .alias("snapshot-compression") // Legacy name used by Solana v1.5.x and older
                .possible_values(SUPPORTED_ARCHIVE_COMPRESSION)
                .default_value(&default_args.snapshot_archive_format)
                .value_name("ARCHIVE_TYPE")
                .takes_value(true)
                .help("Snapshot archive format to use."),
        )
        /// 于限制解压缩后的 Genesis Archive 的最大大小。
        .arg(
            Arg::with_name("max_genesis_archive_unpacked_size")
                .long("max-genesis-archive-unpacked-size")
                .value_name("NUMBER")
                .takes_value(true)
                .default_value(&default_args.genesis_archive_unpacked_size)
                .help("maximum total uncompressed file size of downloaded genesis archive"),
        )
        /// 涉及到节点在启动过程中如何处理 WAL（Write-Ahead Log） 的恢复过程。
        /// WAL 是一种用于确保数据库一致性的日志机制，常用于处理事务和保证数据的持久性。
        /// Solana 使用 WAL 来记录区块链中的重要状态和交易信息，以便在节点崩溃或重启时进行恢复。
        .arg(
            Arg::with_name("wal_recovery_mode")
                .long("wal-recovery-mode")
                .value_name("MODE")
                .takes_value(true)
                .possible_values(&[
                    "tolerate_corrupted_tail_records",
                    "absolute_consistency",
                    "point_in_time",
                    "skip_any_corrupted_record",
                ])
                .help("Mode to recovery the ledger db write ahead log."),
        )
        /// poh 绑定到哪个 cpu 核心
        .arg(
            Arg::with_name("poh_pinned_cpu_core")
                .hidden(hidden_unless_forced())
                .long("experimental-poh-pinned-cpu-core")
                .takes_value(true)
                .value_name("CPU_CORE_INDEX")
                .validator(|s| {
                    let core_index = usize::from_str(&s).map_err(|e| e.to_string())?;
                    let max_index = core_affinity::get_core_ids()
                        .map(|cids| cids.len() - 1)
                        .unwrap_or(0);
                    if core_index > max_index {
                        return Err(format!("core index must be in the range [0, {max_index}]"));
                    }
                    Ok(())
                })
                .help("EXPERIMENTAL: Specify which CPU core PoH is pinned to"),
        )
        /// 控制每次生成 PoH 时，批量计算多少个哈希。
        /// 这个参数直接影响 PoH 的计算速度和资源消耗，因为更大的批次意味着更多的哈希计算，
        /// 从而加快了时间戳生成和交易顺序验证的过程。
        .arg(
            Arg::with_name("poh_hashes_per_batch")
                .hidden(hidden_unless_forced())
                .long("poh-hashes-per-batch")
                .takes_value(true)
                .value_name("NUM")
                .help("Specify hashes per batch in PoH service"),
        )
        // 在默认情况下，Solana 节点会尽可能地同时启动账本处理和其他服务。如果启用了 --process-ledger-before-services，
        // 节点将在启动过程中首先处理账本数据，然后才启动其他服务。这意味着节点在启动时会先完成以下操作：
        // 处理账本数据：加载所有相关的历史区块、交易记录等，确保账本的一致性。
        // 启动服务：处理完账本数据后，节点才开始启动 RPC 服务、Gossip 服务等，以便与其他节点进行交互。
        .arg(
            Arg::with_name("process_ledger_before_services")
                .long("process-ledger-before-services")
                .hidden(hidden_unless_forced())
                .help("Process the local ledger fully before starting networking services"),
        )
        // 启用某种账户索引，"program-id", "spl-token-owner", "spl-token-mint"，可以都开启
        // 帐户索引的主要作用是通过存储帐户公钥（Public Key）与其对应的帐户数据之间的映射关系，
        // 提供对帐户状态的快速查询和访问。每个帐户在 Solana 网络中都有一个唯一的公钥，
        // 该公钥会在帐户索引中映射到该帐户的状态信息（如余额、存储的资产等）。
        .arg(
            Arg::with_name("account_indexes")
                .long("account-index")
                .takes_value(true)
                .multiple(true)
                .possible_values(&["program-id", "spl-token-owner", "spl-token-mint"])
                .value_name("INDEX")
                .help("Enable an accounts index, indexed by the selected account field"),
        )
        /// 账户索引排除key
        .arg(
            Arg::with_name("account_index_exclude_key")
                .long(EXCLUDE_KEY)
                .takes_value(true)
                .validator(is_pubkey)
                .multiple(true)
                .value_name("KEY")
                .help("When account indexes are enabled, exclude this key from the index."),
        )
        /// 账户索引包含key
        .arg(
            Arg::with_name("account_index_include_key")
                .long(INCLUDE_KEY)
                .takes_value(true)
                .validator(is_pubkey)
                .conflicts_with("account_index_exclude_key")
                .multiple(true)
                .value_name("KEY")
                .help(
                    "When account indexes are enabled, only include specific keys in the index. \
                     This overrides --account-index-exclude-key.",
                ),
        )
        /// 此选项让用户在执行帐户数据库的清理操作之前，先验证帐户的引用计数。Solana 网络使用引用计数来追踪帐户的使用情况，确保不会在帐户仍在使用时删除它。
        /// 启用 --accounts-db-verify-refcounts 后，节点将扫描所有的附加向量并检查每个帐户的引用计数是否正确。这有助于确保在执行清理操作时，不会错误地删除仍然有引用的帐户。
        /// 附加向量（Append Vecs）：Solana 的帐户数据库存储在附加向量（append vecs）中，每个附加向量包含多个帐户，每个帐户都有一个引用计数。
        .arg(
            Arg::with_name("accounts_db_verify_refcounts")
                .long("accounts-db-verify-refcounts")
                .help(
                    "Debug option to scan all append vecs and verify account index refcounts \
                     prior to clean",
                )
                .hidden(hidden_unless_forced()),
        )
        /// 扫描账户时过滤
        .arg(
            Arg::with_name("accounts_db_scan_filter_for_shrinking")
                .long("accounts-db-scan-filter-for-shrinking")
                .takes_value(true)
                .possible_values(&["all", "only-abnormal", "only-abnormal-with-verify"])
                .help(
                    "Debug option to use different type of filtering for accounts index scan in \
                    shrinking. \"all\" will scan both in-memory and on-disk accounts index, which is the default. \
                    \"only-abnormal\" will scan in-memory accounts index only for abnormal entries and \
                    skip scanning on-disk accounts index by assuming that on-disk accounts index contains \
                    only normal accounts index entry. \"only-abnormal-with-verify\" is similar to \
                    \"only-abnormal\", which will scan in-memory index for abnormal entries, but will also \
                    verify that on-disk account entries are indeed normal.",
                )
                .hidden(hidden_unless_forced()),
        )
        /// 跳过重写免租金账户
        .arg(
            Arg::with_name("accounts_db_test_skip_rewrites")
                .long("accounts-db-test-skip-rewrites")
                .help(
                    "Debug option to skip rewrites for rent-exempt accounts but still add them in \
                     bank delta hash calculation",
                )
                .hidden(hidden_unless_forced()),
        )
        /// 验证快照银行时不要跳过清理
        .arg(
            Arg::with_name("no_skip_initial_accounts_db_clean")
                .long("no-skip-initial-accounts-db-clean")
                .help("Do not skip the initial cleaning of accounts when verifying snapshot bank")
                .hidden(hidden_unless_forced())
                .conflicts_with("accounts_db_skip_shrink"),
        )
        /// 选择方法将账户文件压缩在一起
        .arg(
            Arg::with_name("accounts_db_squash_storages_method")
                .long("accounts-db-squash-storages-method")
                .value_name("METHOD")
                .takes_value(true)
                .possible_values(&["pack", "append"])
                .help("Squash multiple account storage files together using this method")
                .hidden(hidden_unless_forced()),
        )
        /// 账户存储访问方式，mmap或file
        .arg(
            Arg::with_name("accounts_db_access_storages_method")
                .long("accounts-db-access-storages-method")
                .value_name("METHOD")
                .takes_value(true)
                .possible_values(&["mmap", "file"])
                .help("Access account storages using this method")
        )
        /// 比较早的账户AppendVecs会被压缩
        // 在 Solana 中，**Append Vecs**（附加向量）是一种高效的存储数据结构，用于存储和管理帐户（accounts）的状态。由于 Solana 使用的是基于**帐户**的模型，每个帐户的状态（包括余额、持有的资产等信息）都会被存储在账本（ledger）中。为了高效地管理这些大量的帐户数据，Solana 使用了**Append Vecs**来组织和存储它们。
        // ### Append Vecs 的基本概念：
        // - **Append Vecs** 是 Solana 中存储帐户数据的容器。每个 "Append Vec" 是一个文件或数据块，用于存储多个帐户的数据。
        // - **Append Vecs** 采用追加（append）模式，也就是说，一旦数据写入，它就被“追加”到文件的末尾，而不是覆盖或修改已经存在的数据。这种结构使得对大规模数据进行高效的读取和写入变得更加容易。
        // - 每个 `Append Vec` 文件包含多个帐户的状态，存储每个帐户的数据，如余额、所有者信息、存储的数据等。
        // ### 工作原理：
        // Solana 将帐户数据分布在多个 `Append Vec` 文件中，每个文件会持续增长，新的帐户数据会被添加到文件的末尾。每个 `Append Vec` 文件有一个 **最大大小限制**，当一个文件达到限制时，Solana 会创建一个新的 `Append Vec` 文件，继续存储新的帐户数据。
        // 1. **存储结构**：每个帐户的数据存储在 `Append Vec` 文件中。文件会按顺序写入，不会修改已写入的数据（即使帐户的状态发生变化，新的状态会追加到新的位置）。
        // 2. **高效读取**：由于 Solana 在 `Append Vecs` 中追加数据的特性，节点可以高效地读取这些文件，尤其是在处理大量帐户时。读取一个文件时，系统可以很容易地跳过已被清理的数据部分，直接访问当前有效的数据。
        // 3. **删除与清理**：当帐户不再被使用（比如帐户余额为零并且没有任何关联），Solana 会标记这些帐户为已删除。节点不会立即从文件中删除这些帐户，而是等待一定条件下的清理操作。这个清理过程会扫描 `Append Vecs` 中的无效帐户，并通过整理将不再使用的数据移除。
        // 4. **追加模式（Append Mode）**：在 Solana 中，"Append" 代表每当帐户发生变更时，新数据会被追加到 `Append Vec` 文件中。这避免了多次修改文件的开销，同时也有助于提高性能。
        // ### 主要特点：
        // - **可扩展性**：Append Vecs 使 Solana 能够处理成千上万的帐户数据，并且随着新帐户的加入，系统可以持续扩展。
        // - **高效的磁盘访问**：Append Vecs 使用追加模式，可以减少文件系统中的随机写入操作，这使得磁盘操作更加高效。
        // - **数据清理与压缩**：Solana 通过垃圾回收（Garbage Collection）和数据压缩机制来定期清理无效帐户数据，保证 `Append Vecs` 文件不会无限增长。
        // - **高并发性**：由于 Append Vecs 的设计方式，Solana 能够支持高并发的交易处理，多个帐户可以并行地进行读写操作。
        // ### Append Vecs 的结构：
        // `Append Vec` 文件的结构可以简要地描述为：
        // 1. **文件头**：包含文件的元数据，如版本信息、文件大小、使用的引用计数等。
        // 2. **数据部分**：包含多个帐户的状态数据。每个帐户的状态通常包括帐户余额、持有的资产等信息。
        // 3. **引用计数**：每个帐户在文件中有一个引用计数，用于跟踪该帐户是否仍然被使用。引用计数为零的帐户可以被清理或移除。
        // ### 为什么使用 Append Vecs？
        // - **性能**：Append Vecs 通过追加数据的方式避免了频繁的文件更新和重写，这种结构非常适合大规模数据的存储和管理。Solana 可以快速地处理来自多个用户的请求，同时保证系统的高效性。
        // - **简化存储操作**：由于数据是追加的，Solana 不需要频繁地在文件中查找或修改数据，这使得存储操作更加简单和高效。
        // - **更好的垃圾回收和清理**：Solana 通过垃圾回收机制来清理不再使用的帐户。清理过程会定期扫描和清除不再有效的数据，从而保持存储的高效性。
        // ### 例子：
        // 假设 Solana 有一个包含多个帐户的 `Append Vec` 文件，文件结构可能如下：
        // ```
        // Append Vec 文件：
        // | 文件头 | 帐户数据块1 | 帐户数据块2 | ... | 帐户数据块N |
        // ```
        // 每个帐户数据块包含一个帐户的所有状态信息，比如余额、所有者、持有的资产等。当一个新的帐户被创建时，它的数据会被追加到文件的末尾；当帐户的数据发生变更时，新的状态会追加到文件。
        // ### 总结：
        // `Append Vecs` 是 Solana 用于高效存储和管理帐户数据的关键技术之一。通过采用追加模式，它能够在处理大规模数据时保持高性能，同时简化了数据存储和访问。这个设计使得 Solana 能够在高吞吐量和低延迟的情况下，处理大量的交易和账户数据。
        .arg(
            Arg::with_name("accounts_db_ancient_append_vecs")
                .long("accounts-db-ancient-append-vecs")
                .value_name("SLOT-OFFSET")
                .validator(is_parsable::<i64>)
                .takes_value(true)
                .help(
                    "AppendVecs that are older than (slots_per_epoch - SLOT-OFFSET) are squashed \
                     together.",
                )
                .hidden(hidden_unless_forced()),
        )
        // 这个参数用于设置 Solana 节点中“古老存储”（Ancient Storage）的理想大小。古老存储是指那些经过很长时间没有访问的帐户数据。Solana 为了提高性能和优化存储，
        // 会将这些不常访问的帐户数据分离到“古老存储”中，以便在未来需要时能够快速检索，但不会影响当前的活跃帐户查询性能。
        // 目的：该参数指定古老存储理想的大小，以便节点能够按需管理存储区域。通常，古老存储会包含过时或不常更新的数据，确保活跃数据能够保持较快的访问速度。
        // 配置：这个大小可以通过此参数进行配置，确保 Solana 节点的存储不会因为过多的历史数据而变得低效。
        // 默认值：Solana 使用合理的默认值来平衡存储和性能，但这个值可以根据节点的资源、存储需求和性能要求进行调整。
        .arg(
            Arg::with_name("accounts_db_ancient_storage_ideal_size")
                .long("accounts-db-ancient-storage-ideal-size")
                .value_name("BYTES")
                .validator(is_parsable::<u64>)
                .takes_value(true)
                .help("The smallest size of ideal ancient storage.")
                .hidden(hidden_unless_forced()),
        )
        // 此参数控制节点能够拥有多少个古老存储区。每个存储区可能包含多条历史帐户数据。
        // 如果节点创建了太多古老存储区域，它可能会占用过多的存储资源，影响性能，因此需要通过此参数进行限制。
        .arg(
            Arg::with_name("accounts_db_max_ancient_storages")
                .long("accounts-db-max-ancient-storages")
                .value_name("USIZE")
                .validator(is_parsable::<usize>)
                .takes_value(true)
                .help("The number of ancient storages the ancient slot combining should converge to.")
                .hidden(hidden_unless_forced()),
        )
        // 这个参数设置了用于计算帐户哈希值时，**公钥（Public Key）**值的“桶（bin）”数量。工作原理：
        // 帐户哈希：为了在 Solana 中快速查找帐户数据，系统需要计算每个帐户的公钥哈希值。哈希值被用作索引的键，通过哈希计算可以快速找到帐户的存储位置。
        // 桶（Bins）：哈希桶是一种组织哈希值的方式。为了避免哈希冲突和性能瓶颈，Solana 使用桶的概念，将帐户的哈希值分散到不同的“桶”中。每个桶存储多个哈希值，并对它们进行管理。
        // 通过设置 --accounts-db-hash-calculation-pubkey-bins，用户可以调整 Solana 节点在计算帐户哈希值时使用的桶的数量，从而优化节点在哈希计算和数据存取方面的性能。
        .arg(
            Arg::with_name("accounts_db_hash_calculation_pubkey_bins")
                .long("accounts-db-hash-calculation-pubkey-bins")
                .value_name("USIZE")
                .validator(is_parsable::<usize>)
                .takes_value(true)
                .help("The number of pubkey bins used for accounts hash calculation.")
                .hidden(hidden_unless_forced()),
        )
        /// 账户数据缓存大小
        .arg(
            Arg::with_name("accounts_db_cache_limit_mb")
                .long("accounts-db-cache-limit-mb")
                .value_name("MEGABYTES")
                .validator(is_parsable::<u64>)
                .takes_value(true)
                .help(
                    "How large the write cache for account data can become. If this is exceeded, \
                     the cache is flushed more aggressively.",
                ),
        )
        // 账户数据读缓存大小
        .arg(
            Arg::with_name("accounts_db_read_cache_limit_mb")
                .long("accounts-db-read-cache-limit-mb")
                .value_name("MAX | LOW,HIGH")
                .takes_value(true)
                .min_values(1)
                .max_values(2)
                .multiple(false)
                .require_delimiter(true)
                .help("How large the read cache for account data can become, in mebibytes")
                .long_help(
                    "How large the read cache for account data can become, in mebibytes. \
                     If given a single value, it will be the maximum size for the cache. \
                     If given a pair of values, they will be the low and high watermarks \
                     for the cache. When the cache exceeds the high watermark, entries will \
                     be evicted until the size reaches the low watermark."
                )
                .hidden(hidden_unless_forced()),
        )
        // 启用累加器哈希
        // 在传统的哈希计算中，每个帐户的哈希值是独立计算的，直接基于帐户的公钥生成哈希值。然而，随着帐户数量的增加，哈希值可能会发生冲突，导致多个帐户具有相同的哈希值或频繁碰撞，从而影响查询性能。
        // 累加器哈希通过使用一个更复杂的哈希结构来减少这种碰撞。累加器是一种数据结构，可以将多个值（如多个帐户的哈希值）合并为一个单一的哈希值。使用这种结构可以有效地减少哈希冲突，提升查询性能，尤其是在大规模数据集上。
        .arg(
            Arg::with_name("accounts_db_experimental_accumulator_hash")
                .long("accounts-db-experimental-accumulator-hash")
                .help("Enables the experimental accumulator hash")
                .hidden(hidden_unless_forced()),
        )
        // 验证累加器哈希
        .arg(
            Arg::with_name("accounts_db_verify_experimental_accumulator_hash")
                .long("accounts-db-verify-experimental-accumulator-hash")
                .help("Verifies the experimental accumulator hash")
                .hidden(hidden_unless_forced()),
        )
        // 快照使用累加器哈希
        .arg(
            Arg::with_name("accounts_db_snapshots_use_experimental_accumulator_hash")
                .long("accounts-db-snapshots-use-experimental-accumulator-hash")
                .help("Snapshots use the experimental accumulator hash")
                .hidden(hidden_unless_forced()),
        )
        // 账户索引扫描结果大小限制
        .arg(
            Arg::with_name("accounts_index_scan_results_limit_mb")
                .long("accounts-index-scan-results-limit-mb")
                .value_name("MEGABYTES")
                .validator(is_parsable::<usize>)
                .takes_value(true)
                .help(
                    "How large accumulated results from an accounts index scan can become. If \
                     this is exceeded, the scan aborts.",
                ),
        )
        /// 账户索引划分到的箱数
        .arg(
            Arg::with_name("accounts_index_bins")
                .long("accounts-index-bins")
                .value_name("BINS")
                .validator(is_pow2)
                .takes_value(true)
                .help("Number of bins to divide the accounts index into"),
        )
        /// 账户索引存储路径
        .arg(
            Arg::with_name("accounts_index_path")
                .long("accounts-index-path")
                .value_name("PATH")
                .takes_value(true)
                .multiple(true)
                .help(
                    "Persistent accounts-index location. \
                    May be specified multiple times. \
                    [default: <LEDGER>/accounts_index]",
                ),
        )
        /// --accounts-db-test-hash-calculation 参数的作用是启用或禁用哈希计算的测试功能。
        /// 它并不直接影响 Solana 节点的生产环境行为，而是用于验证和调试哈希计算的过程，确保帐户数据能够正确映射到哈希表中。
        /// 这个功能通常是在开发和测试环境中使用，帮助开发人员确认哈希计算部分的准确性和效率。
        .arg(
            Arg::with_name("accounts_db_test_hash_calculation")
                .long("accounts-db-test-hash-calculation")
                .help(
                    "Enables testing of hash calculation using stores in AccountsHashVerifier. \
                     This has a computational cost.",
                ),
        )
        /// 压缩稀疏账户来减少空间占用
        .arg(
            Arg::with_name("accounts_shrink_optimize_total_space")
                .long("accounts-shrink-optimize-total-space")
                .takes_value(true)
                .value_name("BOOLEAN")
                .default_value(&default_args.accounts_shrink_optimize_total_space)
                .help(
                    "When this is set to true, the system will shrink the most sparse accounts \
                     and when the overall shrink ratio is above the specified \
                     accounts-shrink-ratio, the shrink will stop and it will skip all other less \
                     sparse accounts.",
                ),
        )
        /// 账户压缩比
        .arg(
            Arg::with_name("accounts_shrink_ratio")
                .long("accounts-shrink-ratio")
                .takes_value(true)
                .value_name("RATIO")
                .default_value(&default_args.accounts_shrink_ratio)
                .help(
                    "Specifies the shrink ratio for the accounts to be shrunk. The shrink ratio \
                     is defined as the ratio of the bytes alive over the  total bytes used. If \
                     the account's shrink ratio is less than this ratio it becomes a candidate \
                     for shrinking. The value must between 0. and 1.0 inclusive.",
                ),
        )
        /// 允许连接到私有地址
        .arg(
            Arg::with_name("allow_private_addr")
                .long("allow-private-addr")
                .takes_value(false)
                .help("Allow contacting private ip addresses")
                .hidden(hidden_unless_forced()),
        )
        /// 日志最大大小限制
        .arg(
            Arg::with_name("log_messages_bytes_limit")
                .long("log-messages-bytes-limit")
                .takes_value(true)
                .validator(is_parsable::<usize>)
                .value_name("BYTES")
                .help("Maximum number of bytes written to the program log before truncation"),
        )
        /// banking trace 大小限制
        /// Banking Trace 是一种跟踪机制，用于记录银行状态在交易执行过程中的变化。
        /// 它提供了对银行状态变动的详细追踪，可以帮助开发者、节点管理员和测试人员深入了解系统在处理交易时是如何更新帐户状态的。
        .arg(
            Arg::with_name("banking_trace_dir_byte_limit")
                // expose friendly alternative name to cli than internal
                // implementation-oriented one
                .long("enable-banking-trace")
                .value_name("BYTES")
                .validator(is_parsable::<DirByteLimit>)
                .takes_value(true)
                // Firstly, zero limit value causes tracer to be disabled
                // altogether, intuitively. On the other hand, this non-zero
                // default doesn't enable banking tracer unless this flag is
                // explicitly given, similar to --limit-ledger-size.
                // see configure_banking_trace_dir_byte_limit() for this.
                .default_value(&default_args.banking_trace_dir_byte_limit)
                .help(
                    "Enables the banking trace explicitly, which is enabled by default and writes \
                     trace files for simulate-leader-blocks, retaining up to the default or \
                     specified total bytes in the ledger. This flag can be used to override its \
                     byte limit.",
                ),
        )
        /// 禁用 banking trace
        .arg(
            Arg::with_name("disable_banking_trace")
                .long("disable-banking-trace")
                .conflicts_with("banking_trace_dir_byte_limit")
                .takes_value(false)
                .help("Disables the banking trace"),
        )
        /// 它用于控制在区块链的领导者（leader）创建新区块时，是否等待一个尚未确认的分叉区块（pending fork）完成重放。这是在多个区块链分叉的情况下，确保新领导者区块与其他可能正在重放的区块位于相同的分叉上的一种机制。
        /// 多个验证者（validator）节点同时处理交易并创建新区块。如果一个验证者节点在处理新区块时，另一个区块链分叉（即从当前分叉派生出来的区块）还在等待重放（即其状态尚未确认），该节点可能会遇到分叉问题。
        /// 这个参数通过控制新区块创建的时机，解决了由于“等待重放”可能导致的分叉冲突。如果不启用此参数，新的领导者区块有可能与当前正在重放的区块位于不同的分叉上，导致冲突和潜在的链选择问题（即可能会确认错误的分叉）。
        .arg(
            Arg::with_name("delay_leader_block_for_pending_fork")
                .hidden(hidden_unless_forced())
                .long("delay-leader-block-for-pending-fork")
                .takes_value(false)
                .help(
                    "Delay leader block creation while replaying a block which descends from the \
                    current fork and has a lower slot than our next leader slot. If we don't \
                    delay here, our new leader block will be on a different fork from the \
                    block we are replaying and there is a high chance that the cluster will \
                    confirm that block's fork rather than our leader block's fork because it \
                    was created before we started creating ours.",
                ),
        )
        /// 块验证方法， "blockstore-processor", "unified-scheduler"
        .arg(
            Arg::with_name("block_verification_method")
                .long("block-verification-method")
                .value_name("METHOD")
                .takes_value(true)
                .possible_values(BlockVerificationMethod::cli_names())
                .help(BlockVerificationMethod::cli_message()),
        )
        /// 块生产方法， central-scheduler
        .arg(
            Arg::with_name("block_production_method")
                .long("block-production-method")
                .value_name("METHOD")
                .takes_value(true)
                .possible_values(BlockProductionMethod::cli_names())
                .help(BlockProductionMethod::cli_message()),
        )
        /// 统一调度事务线程数
        .arg(
            Arg::with_name("unified_scheduler_handler_threads")
                .long("unified-scheduler-handler-threads")
                .value_name("COUNT")
                .takes_value(true)
                .validator(|s| is_within_range(s, 1..))
                .help(DefaultSchedulerPool::cli_message()),
        )
        // `--wen-restart` 是 Solana 节点的一个配置参数，用于协调集群重启过程中的验证器行为。这个参数通常只在 **协调集群重启**（coordinated cluster restart）过程中使用，在这个模式下，验证器会进入 **Wen Restart 模式**，暂停正常活动，并协同其他验证器选择一个安全的重启位置。
        // ### 详细解释：
        // 在 Solana 集群中，当出现需要协调的重启时，所有的验证器节点会协作选择一个合适的“安全重启槽”（safe restart slot）作为重启的基准点。这个过程可以确保重启时不会回滚任何已经乐观确认的槽（optimistically confirmed slots）。
        // 启用 `--wen-restart` 参数后，验证器会进入 Wen Restart 模式，暂停其正常的活动，专注于完成重启过程中的协作工作。以下是该参数的详细功能和流程：
        // ### 主要功能：
        // 1. **进入 Wen Restart 模式**：当启用该参数时，验证器会暂停正常的交易和投票等活动，专门进入协调重启的模式。在这个模式下，验证器会将其最后的投票信息广播给其他节点，以便达成共识，选择一个安全的重启槽。
        // 2. **达成共识选定重启槽**：所有验证器将根据其状态达成共识，选择一个 "安全重启槽"。这个槽将是最新的乐观确认槽（optimistically confirmed slot）的后代，保证在重启时不会丢失已经确认的状态。
        // 3. **修复选定分叉上的区块**：在安全重启槽确定后，验证器将修复该槽的所有区块，确保系统在重启后的一致性。
        // 4. **保存进度文件**：重启过程中的进度将被保存到指定的文件中（该文件使用 `proto3` 格式）。如果集群重启顺利完成，验证器会退出并返回状态代码 200，表示成功。之后，操作员需要根据退出时提供的错误日志信息，使用 `--wait_for_supermajority` 等参数重启验证器，以便恢复集群的正常运行。
        // 5. **故障排除和调试**：如果 `wen_restart` 失败，可以通过查看保存的进度文件来进行调试，文件中包含详细的操作日志。操作员还可以参考 Solana 的 Discord 频道中的指示进行进一步操作。
        // ### 参数的关键点：
        // - **依赖关系**：该参数依赖于 `--wen_restart_coordinator` 参数（协调器模式）。它还与 `--wait_for_supermajority` 参数存在冲突，后者通常在重启完成后使用。
        // - **使用要求**：使用此参数时，还需要指定领导者的公钥（`--wen-restart-leader`），以便明确参与协作的领导者。
        // - **进度文件**：进度文件会记录集群重启的进度，并可用于调试。该文件通常包含在重启失败时用于排查问题的详细信息。
        // ### 使用场景：
        // - **协调重启**：当 Solana 集群需要进行协调重启时，启用 `--wen-restart` 可以确保所有验证器一致性地选择一个安全的重启槽，并在该槽上修复区块。
        // - **调试重启问题**：当重启过程中出现问题时，进度文件可以提供详细的日志，帮助开发者和操作员进行调试。
        // ### 示例：
        // ```bash
        // --wen-restart /path/to/progress/file
        // ```
        // 启用此参数时，验证器会在协调重启过程中记录重启进度到指定的文件中。
        // ### 总结：
        // `--wen-restart` 是 Solana 节点的一项功能，用于协调集群重启过程中的验证器行为。它通过暂停正常活动并与其他验证器达成共识，选择一个安全的重启槽，确保集群在重启时的一致性和数据完整性。重启进度将被记录在指定的文件中，便于调试和排查问题。如果重启成功，验证器会自动退出，并等待后续的集群恢复操作。
        .arg(
            Arg::with_name("wen_restart")
                .long("wen-restart")
                .hidden(hidden_unless_forced())
                .value_name("FILE")
                .takes_value(true)
                .required(false)
                .conflicts_with("wait_for_supermajority")
                .requires("wen_restart_coordinator")
                .help(
                    "Only used during coordinated cluster restarts.\
                    \n\n\
                    Need to also specify the leader's pubkey in --wen-restart-leader.\
                    \n\n\
                    When specified, the validator will enter Wen Restart mode which \
                    pauses normal activity. Validators in this mode will gossip their last \
                    vote to reach consensus on a safe restart slot and repair all blocks \
                    on the selected fork. The safe slot will be a descendant of the latest \
                    optimistically confirmed slot to ensure we do not roll back any \
                    optimistically confirmed slots. \
                    \n\n\
                    The progress in this mode will be saved in the file location provided. \
                    If consensus is reached, the validator will automatically exit with 200 \
                    status code. Then the operators are expected to restart the validator \
                    with --wait_for_supermajority and other arguments (including new shred_version, \
                    supermajority slot, and bankhash) given in the error log before the exit so \
                    the cluster will resume execution. The progress file will be kept around \
                    for future debugging. \
                    \n\n\
                    If wen_restart fails, refer to the progress file (in proto3 format) for \
                    further debugging and watch the discord channel for instructions.",
                ),
        )
        /// 设置 wen restart 中心协调器
        .arg(
            Arg::with_name("wen_restart_coordinator")
                .long("wen-restart-coordinator")
                .hidden(hidden_unless_forced())
                .value_name("PUBKEY")
                .takes_value(true)
                .required(false)
                .requires("wen_restart")
                .help(
                    "Specifies the pubkey of the leader used in wen restart. \
                    May get stuck if the leader used is different from others.",
                ),
        )
        /// 线程配置
        .args(&thread_args(&default_args.thread_args))
        /// 弃用的配置
        .args(&get_deprecated_arguments())
        /// 附加提示
        .after_help("The default subcommand is run")
        .subcommand(
            /// 退出命令，会找合适的时机优雅退出
            SubCommand::with_name("exit")
                .about("Send an exit request to the validator")
                /// 强制退出
                .arg(
                    Arg::with_name("force")
                        .short("f")
                        .long("force")
                        .takes_value(false)
                        .help(
                            "Request the validator exit immediately instead of waiting for a \
                             restart window",
                        ),
                )
                /// 监控退出状态
                .arg(
                    Arg::with_name("monitor")
                        .short("m")
                        .long("monitor")
                        .takes_value(false)
                        .help("Monitor the validator after sending the exit request"),
                )
                /// 在轮到自己成为 leader 前的时间，马上就要当leader了就不关
                .arg(
                    Arg::with_name("min_idle_time")
                        .long("min-idle-time")
                        .takes_value(true)
                        .validator(is_parsable::<usize>)
                        .value_name("MINUTES")
                        .default_value(&default_args.exit_min_idle_time)
                        .help(
                            "Minimum time that the validator should not be leader before \
                             restarting",
                        ),
                )
                /// 最大过失质押，损失太大就不关
                .arg(
                    Arg::with_name("max_delinquent_stake")
                        .long("max-delinquent-stake")
                        .takes_value(true)
                        .validator(is_valid_percentage)
                        .default_value(&default_args.exit_max_delinquent_stake)
                        .value_name("PERCENT")
                        .help("The maximum delinquent stake % permitted for an exit"),
                )
                /// 跳过快照检查
                .arg(
                    Arg::with_name("skip_new_snapshot_check")
                        .long("skip-new-snapshot-check")
                        .help("Skip check for a new snapshot"),
                )
                /// 跳过健康检查
                .arg(
                    Arg::with_name("skip_health_check")
                        .long("skip-health-check")
                        .help("Skip health check"),
                ),
        )
        .subcommand(
            /// 调整验证器的授权投票人
            SubCommand::with_name("authorized-voter")
                .about("Adjust the validator authorized voters")
                /// 没有用下一级子命令时，显示帮助文本
                .setting(AppSettings::SubcommandRequiredElseHelp)
                /// 子命令前缀匹配
                .setting(AppSettings::InferSubcommands)
                .subcommand(
                    /// 增加授权投票人
                    SubCommand::with_name("add")
                        .about("Add an authorized voter")
                        .arg(
                            Arg::with_name("authorized_voter_keypair")
                                .index(1)
                                .value_name("KEYPAIR")
                                .required(false)
                                .takes_value(true)
                                .validator(is_keypair)
                                .help(
                                    "Path to keypair of the authorized voter to add [default: \
                                     read JSON keypair from stdin]",
                                ),
                        )
                        .after_help(
                            "Note: the new authorized voter only applies to the currently running \
                             validator instance",
                        ),
                )
                .subcommand(
                    /// 移除所有授权投票人
                    SubCommand::with_name("remove-all")
                        .about("Remove all authorized voters")
                        .after_help(
                            "Note: the removal only applies to the currently running validator \
                             instance",
                        ),
                ),
        )
        .subcommand(
            /// 显示验证器联系信息
            SubCommand::with_name("contact-info")
                .about("Display the validator's contact info")
                .arg(
                    /// 输出格式
                    Arg::with_name("output")
                        .long("output")
                        .takes_value(true)
                        .value_name("MODE")
                        .possible_values(&["json", "json-compact"])
                        .help("Output display mode"),
                ),
        )
        .subcommand(
            /// 从指定节点拉取信息，修复消息分片或时隙
            SubCommand::with_name("repair-shred-from-peer")
                .about("Request a repair from the specified validator")
                .arg(
                    /// 指定节点
                    Arg::with_name("pubkey")
                        .long("pubkey")
                        .value_name("PUBKEY")
                        .required(false)
                        .takes_value(true)
                        .validator(is_pubkey)
                        .help("Identity pubkey of the validator to repair from"),
                )
                .arg(
                    /// 时隙
                    Arg::with_name("slot")
                        .long("slot")
                        .value_name("SLOT")
                        .takes_value(true)
                        .validator(is_parsable::<u64>)
                        .help("Slot to repair"),
                )
                .arg(
                    /// 消息分片
                    Arg::with_name("shred")
                        .long("shred")
                        .value_name("SHRED")
                        .takes_value(true)
                        .validator(is_parsable::<u64>)
                        .help("Shred to repair"),
                ),
        )
        .subcommand(
            /// 调整白名单
            SubCommand::with_name("repair-whitelist")
                .about("Manage the validator's repair protocol whitelist")
                .setting(AppSettings::SubcommandRequiredElseHelp)
                .setting(AppSettings::InferSubcommands)
                .subcommand(
                    /// 获取白名单
                    SubCommand::with_name("get")
                        .about("Display the validator's repair protocol whitelist")
                        .arg(
                            Arg::with_name("output")
                                .long("output")
                                .takes_value(true)
                                .value_name("MODE")
                                .possible_values(&["json", "json-compact"])
                                .help("Output display mode"),
                        ),
                )
                .subcommand(
                    /// 设置白名单
                    SubCommand::with_name("set")
                        .about("Set the validator's repair protocol whitelist")
                        .setting(AppSettings::ArgRequiredElseHelp)
                        .arg(
                            Arg::with_name("whitelist")
                                .long("whitelist")
                                .validator(is_pubkey)
                                .value_name("VALIDATOR IDENTITY")
                                .multiple(true)
                                .takes_value(true)
                                .help("Set the validator's repair protocol whitelist"),
                        )
                        .after_help(
                            "Note: repair protocol whitelist changes only apply to the currently \
                             running validator instance",
                        ),
                )
                .subcommand(
                    /// 移除所有白名单
                    SubCommand::with_name("remove-all")
                        .about("Clear the validator's repair protocol whitelist")
                        .after_help(
                            "Note: repair protocol whitelist changes only apply to the currently \
                             running validator instance",
                        ),
                ),
        )
        .subcommand(
            /// 初始化节点
            SubCommand::with_name("init").about("Initialize the ledger directory then exit"),
        )
        /// 监控节点
        .subcommand(SubCommand::with_name("monitor").about("Monitor the validator"))
        /// 运行节点
        .subcommand(SubCommand::with_name("run").about("Run the validator"))
        .subcommand(
            /// 管理 geyser 插件
            SubCommand::with_name("plugin")
                .about("Manage and view geyser plugins")
                .setting(AppSettings::SubcommandRequiredElseHelp)
                .setting(AppSettings::InferSubcommands)
                .subcommand(
                    /// 列出插件
                    SubCommand::with_name("list").about("List all current running gesyer plugins"),
                )
                .subcommand(
                    /// 上传插件
                    SubCommand::with_name("unload")
                        .about(
                            "Unload a particular gesyer plugin. You must specify the gesyer \
                             plugin name",
                        )
                        .arg(Arg::with_name("name").required(true).takes_value(true)),
                )
                .subcommand(
                    /// 重新加载插件
                    SubCommand::with_name("reload")
                        .about(
                            "Reload a particular gesyer plugin. You must specify the gesyer \
                             plugin name and the new config path",
                        )
                        .arg(Arg::with_name("name").required(true).takes_value(true))
                        .arg(Arg::with_name("config").required(true).takes_value(true)),
                )
                .subcommand(
                    /// 加载插件
                    SubCommand::with_name("load")
                        .about(
                            "Load a new gesyer plugin. You must specify the config path. Fails if \
                             overwriting (use reload)",
                        )
                        .arg(Arg::with_name("config").required(true).takes_value(true)),
                ),
        )
        .subcommand(
            /// 设置节点公钥
            SubCommand::with_name("set-identity")
                .about("Set the validator identity")
                .arg(
                    /// 密钥对
                    Arg::with_name("identity")
                        .index(1)
                        .value_name("KEYPAIR")
                        .required(false)
                        .takes_value(true)
                        .validator(is_keypair)
                        .help(
                            "Path to validator identity keypair [default: read JSON keypair from \
                             stdin]",
                        ),
                )
                .arg(
                    /// 需要塔式共识状态
                    clap::Arg::with_name("require_tower")
                        .long("require-tower")
                        .takes_value(false)
                        .help(
                            "Refuse to set the validator identity if saved tower state is not \
                             found",
                        ),
                )
                /// 新公钥设置只会影响当前运行中的验证器
                .after_help(
                    "Note: the new identity only applies to the currently running validator \
                     instance",
                ),
        )
        .subcommand(
            /// 设置日志过滤器， RUST_LOG 格式
            SubCommand::with_name("set-log-filter")
                .about("Adjust the validator log filter")
                .arg(
                    /// 过滤器
                    Arg::with_name("filter").takes_value(true).index(1).help(
                        "New filter using the same format as the RUST_LOG environment variable",
                    ),
                )
                .after_help(
                    "Note: the new filter only applies to the currently running validator instance",
                ),
        )
        .subcommand(
            /// 允许你为某些节点设置自定义的 验证器权益。它通常用于调试、测试、或者在集群的某些验证器节点需要特殊配置时进行调整。
            /// 例如，在集群启动或进行某些重要操作时，可能需要指定某些验证器的特殊权益配置，或者在调试期间临时更改某些验证器的状态。
            SubCommand::with_name("staked-nodes-overrides")
                .about("Overrides stakes of specific node identities.")
                .arg(
                    Arg::with_name("path")
                        .value_name("PATH")
                        .takes_value(true)
                        .required(true)
                        .help(
                            "Provide path to a file with custom overrides for stakes of specific \
                             validator identities.",
                        ),
                )
                .after_help(
                    "Note: the new staked nodes overrides only applies to the currently running \
                     validator instance",
                ),
        )
        .subcommand(
            /// 等待重启窗口
            SubCommand::with_name("wait-for-restart-window")
                .about("Monitor the validator for a good time to restart")
                .arg(
                    /// 距离下次任期最少要间隔多久
                    Arg::with_name("min_idle_time")
                        .long("min-idle-time")
                        .takes_value(true)
                        .validator(is_parsable::<usize>)
                        .value_name("MINUTES")
                        .default_value(&default_args.wait_for_restart_window_min_idle_time)
                        .help(
                            "Minimum time that the validator should not be leader before \
                             restarting",
                        ),
                )
                .arg(
                    /// 节点公钥
                    Arg::with_name("identity")
                        .long("identity")
                        .value_name("ADDRESS")
                        .takes_value(true)
                        .validator(is_pubkey_or_keypair)
                        .help("Validator identity to monitor [default: your validator]"),
                )
                .arg(
                    /// 最大过失质押
                    Arg::with_name("max_delinquent_stake")
                        .long("max-delinquent-stake")
                        .takes_value(true)
                        .validator(is_valid_percentage)
                        .default_value(&default_args.wait_for_restart_window_max_delinquent_stake)
                        .value_name("PERCENT")
                        .help("The maximum delinquent stake % permitted for a restart"),
                )
                .arg(
                    /// 跳过快照检查
                    Arg::with_name("skip_new_snapshot_check")
                        .long("skip-new-snapshot-check")
                        .help("Skip check for a new snapshot"),
                )
                .arg(
                    /// 跳过健康检查
                    Arg::with_name("skip_health_check")
                        .long("skip-health-check")
                        .help("Skip health check"),
                )
                /// 如果非 0 退出码，说明不适合重启
                .after_help(
                    "Note: If this command exits with a non-zero status then this not a good time \
                     for a restart",
                ),
        )
        .subcommand(
            /// 设置公共地址
            SubCommand::with_name("set-public-address")
                .about("Specify addresses to advertise in gossip")
                .arg(
                    /// tpu地址
                    Arg::with_name("tpu_addr")
                        .long("tpu")
                        .value_name("HOST:PORT")
                        .takes_value(true)
                        .validator(solana_net_utils::is_host_port)
                        .help("TPU address to advertise in gossip"),
                )
                .arg(
                    /// tpu 转发地址
                    Arg::with_name("tpu_forwards_addr")
                        .long("tpu-forwards")
                        .value_name("HOST:PORT")
                        .takes_value(true)
                        .validator(solana_net_utils::is_host_port)
                        .help("TPU Forwards address to advertise in gossip"),
                )
                .group(
                    /// 分组，上面两个必须二选一或都选
                    ArgGroup::with_name("set_public_address_details")
                        .args(&["tpu_addr", "tpu_forwards_addr"])
                        .required(true)
                        .multiple(true),
                )
                .after_help("Note: At least one arg must be used. Using multiple is ok"),
        );
}

/// Deprecated argument description should be moved into the [`deprecated_arguments()`] function,
/// expressed as an instance of this type.
/// 已弃用的参数，不看了
struct DeprecatedArg {
    /// Deprecated argument description, moved here as is.
    ///
    /// `hidden` property will be modified by [`deprecated_arguments()`] to only show this argument
    /// if [`hidden_unless_forced()`] says they should be displayed.
    arg: Arg<'static, 'static>,

    /// If simply replaced by a different argument, this is the name of the replacement.
    ///
    /// Content should be an argument name, as presented to users.
    replaced_by: Option<&'static str>,

    /// An explanation to be shown to the user if they still use this argument.
    ///
    /// Content should be a complete sentence or several, ending with a period.
    usage_warning: Option<&'static str>,
}

/// 已弃用的参数，不看了
fn deprecated_arguments() -> Vec<DeprecatedArg> {
    let mut res = vec![];

    // This macro reduces indentation and removes some noise from the argument declaration list.
    macro_rules! add_arg {
        (
            $arg:expr
            $( , replaced_by: $replaced_by:expr )?
            $( , usage_warning: $usage_warning:expr )?
            $(,)?
        ) => {
            let replaced_by = add_arg!(@into-option $( $replaced_by )?);
            let usage_warning = add_arg!(@into-option $( $usage_warning )?);
            res.push(DeprecatedArg {
                arg: $arg,
                replaced_by,
                usage_warning,
            });
        };

        (@into-option) => { None };
        (@into-option $v:expr) => { Some($v) };
    }

    add_arg!(
        Arg::with_name("accounts_db_skip_shrink")
            .long("accounts-db-skip-shrink")
            .help("Enables faster starting of validators by skipping startup clean and shrink."),
        usage_warning: "Enabled by default",
    );
    add_arg!(Arg::with_name("accounts_hash_interval_slots")
        .long("accounts-hash-interval-slots")
        .value_name("NUMBER")
        .takes_value(true)
        .help("Number of slots between verifying accounts hash.")
        .validator(|val| {
            if val.eq("0") {
                Err(String::from("Accounts hash interval cannot be zero"))
            } else {
                Ok(())
            }
        }));
    // deprecated in v2.1 by PR #2721
    add_arg!(Arg::with_name("accounts_index_memory_limit_mb")
        .long("accounts-index-memory-limit-mb")
        .value_name("MEGABYTES")
        .validator(is_parsable::<usize>)
        .takes_value(true)
        .help(
            "How much memory the accounts index can consume. If this is exceeded, some \
         account index entries will be stored on disk.",
        ),
        usage_warning: "index memory limit has been deprecated. The limit arg has no effect now.",
    );
    add_arg!(Arg::with_name("accountsdb_repl_bind_address")
        .long("accountsdb-repl-bind-address")
        .value_name("HOST")
        .takes_value(true)
        .validator(solana_net_utils::is_host)
        .help(
            "IP address to bind the AccountsDb Replication port [default: use \
                     --bind-address]",
        ));
    add_arg!(Arg::with_name("accountsdb_repl_port")
        .long("accountsdb-repl-port")
        .value_name("PORT")
        .takes_value(true)
        .validator(port_validator)
        .help("Enable AccountsDb Replication Service on this port"));
    add_arg!(Arg::with_name("accountsdb_repl_threads")
        .long("accountsdb-repl-threads")
        .value_name("NUMBER")
        .validator(is_parsable::<usize>)
        .takes_value(true)
        .help("Number of threads to use for servicing AccountsDb Replication requests"));
    add_arg!(Arg::with_name("disable_accounts_disk_index")
        .long("disable-accounts-disk-index")
        .help("Disable the disk-based accounts index if it is enabled by default.")
        .conflicts_with("accounts_index_memory_limit_mb"));
    add_arg!(
        Arg::with_name("disable_quic_servers")
            .long("disable-quic-servers")
            .takes_value(false),
        usage_warning: "The quic server cannot be disabled.",
    );
    add_arg!(Arg::with_name("enable_accountsdb_repl")
        .long("enable-accountsdb-repl")
        .takes_value(false)
        .help("Enable AccountsDb Replication"));
    add_arg!(
        Arg::with_name("enable_cpi_and_log_storage")
            .long("enable-cpi-and-log-storage")
            .requires("enable_rpc_transaction_history")
            .takes_value(false)
            .help(
                "Include CPI inner instructions, logs and return data in the historical \
                 transaction info stored",
            ),
        replaced_by: "enable-extended-tx-metadata-storage",
    );
    add_arg!(
        Arg::with_name("enable_quic_servers")
            .long("enable-quic-servers"),
        usage_warning: "The quic server is now enabled by default.",
    );
    add_arg!(Arg::with_name("minimal_rpc_api")
        .long("minimal-rpc-api")
        .takes_value(false)
        .help("Only expose the RPC methods required to serve snapshots to other nodes"));
    add_arg!(
        Arg::with_name("no_check_vote_account")
            .long("no-check-vote-account")
            .takes_value(false)
            .conflicts_with("no_voting")
            .requires("entrypoint")
            .help("Skip the RPC vote account sanity check"),
        usage_warning: "Vote account sanity checks are no longer performed by default.",
    );
    add_arg!(Arg::with_name("no_rocksdb_compaction")
        .long("no-rocksdb-compaction")
        .takes_value(false)
        .help("Disable manual compaction of the ledger database"));
    add_arg!(
        Arg::with_name("replay_slots_concurrently")
            .long("replay-slots-concurrently")
            .help("Allow concurrent replay of slots on different forks")
            .conflicts_with("replay_forks_threads"),
        replaced_by: "replay_forks_threads",
        usage_warning: "Equivalent behavior to this flag would be --replay-forks-threads 4");
    add_arg!(Arg::with_name("rocksdb_compaction_interval")
        .long("rocksdb-compaction-interval-slots")
        .value_name("ROCKSDB_COMPACTION_INTERVAL_SLOTS")
        .takes_value(true)
        .help("Number of slots between compacting ledger"));
    // Deprecated in v2.2
    add_arg!(Arg::with_name("rocksdb_fifo_shred_storage_size")
        .long("rocksdb-fifo-shred-storage-size")
        .value_name("SHRED_STORAGE_SIZE_BYTES")
        .takes_value(true)
        .validator(is_parsable::<u64>)
        .help(
            "The shred storage size in bytes. The suggested value is at least 50% of your ledger \
             storage size. If this argument is unspecified, we will assign a proper value based \
             on --limit-ledger-size. If --limit-ledger-size is not presented, it means there is \
             no limitation on the ledger size and thus rocksdb_fifo_shred_storage_size will also \
             be unbounded.",
        ));
    add_arg!(Arg::with_name("rocksdb_max_compaction_jitter")
        .long("rocksdb-max-compaction-jitter-slots")
        .value_name("ROCKSDB_MAX_COMPACTION_JITTER_SLOTS")
        .takes_value(true)
        .help("Introduce jitter into the compaction to offset compaction operation"));
    add_arg!(Arg::with_name("rpc_pubsub_max_connections")
        .long("rpc-pubsub-max-connections")
        .value_name("NUMBER")
        .takes_value(true)
        .validator(is_parsable::<usize>)
        .help(
            "The maximum number of connections that RPC PubSub will support. This is a \
             hard limit and no new connections beyond this limit can be made until an old \
             connection is dropped."
        ));
    add_arg!(Arg::with_name("rpc_pubsub_max_fragment_size")
        .long("rpc-pubsub-max-fragment-size")
        .value_name("BYTES")
        .takes_value(true)
        .validator(is_parsable::<usize>)
        .help(
            "The maximum length in bytes of acceptable incoming frames. Messages longer \
             than this will be rejected"
        ));
    add_arg!(Arg::with_name("rpc_pubsub_max_in_buffer_capacity")
        .long("rpc-pubsub-max-in-buffer-capacity")
        .value_name("BYTES")
        .takes_value(true)
        .validator(is_parsable::<usize>)
        .help("The maximum size in bytes to which the incoming websocket buffer can grow."));
    add_arg!(Arg::with_name("rpc_pubsub_max_out_buffer_capacity")
        .long("rpc-pubsub-max-out-buffer-capacity")
        .value_name("BYTES")
        .takes_value(true)
        .validator(is_parsable::<usize>)
        .help("The maximum size in bytes to which the outgoing websocket buffer can grow."));
    add_arg!(
        Arg::with_name("skip_poh_verify")
            .long("skip-poh-verify")
            .takes_value(false)
            .help("Skip ledger verification at validator bootup."),
        replaced_by: "skip-startup-ledger-verification",
    );

    res
}

// Helper to add arguments that are no longer used but are being kept around to avoid breaking
// validator startup commands.
/// 获取弃用的参数
fn get_deprecated_arguments() -> Vec<Arg<'static, 'static>> {
    deprecated_arguments()
        .into_iter()
        .map(|info| {
            let arg = info.arg;
            // Hide all deprecated arguments by default.
            arg.hidden(hidden_unless_forced())
        })
        .collect()
}

/// 警告参数弃用
pub fn warn_for_deprecated_arguments(matches: &ArgMatches) {
    for DeprecatedArg {
        arg,
        replaced_by,
        usage_warning,
    } in deprecated_arguments().into_iter()
    {
        if matches.is_present(arg.b.name) {
            let mut msg = format!("--{} is deprecated", arg.b.name.replace('_', "-"));
            if let Some(replaced_by) = replaced_by {
                msg.push_str(&format!(", please use --{replaced_by}"));
            }
            msg.push('.');
            if let Some(usage_warning) = usage_warning {
                msg.push_str(&format!("  {usage_warning}"));
                if !msg.ends_with('.') {
                    msg.push('.');
                }
            }
            warn!("{}", msg);
        }
    }
}

pub struct DefaultArgs {
    pub bind_address: String,
    pub dynamic_port_range: String,
    pub ledger_path: String,

    pub genesis_archive_unpacked_size: String,
    pub health_check_slot_distance: String,
    pub tower_storage: String,
    pub etcd_domain_name: String,
    pub send_transaction_service_config: send_transaction_service::Config,

    pub rpc_max_multiple_accounts: String,
    pub rpc_pubsub_max_active_subscriptions: String,
    pub rpc_pubsub_queue_capacity_items: String,
    pub rpc_pubsub_queue_capacity_bytes: String,
    pub rpc_send_transaction_retry_ms: String,
    pub rpc_send_transaction_batch_ms: String,
    pub rpc_send_transaction_leader_forward_count: String,
    pub rpc_send_transaction_service_max_retries: String,
    pub rpc_send_transaction_batch_size: String,
    pub rpc_send_transaction_retry_pool_max_size: String,
    pub rpc_threads: String,
    pub rpc_blocking_threads: String,
    pub rpc_niceness_adjustment: String,
    pub rpc_bigtable_timeout: String,
    pub rpc_bigtable_instance_name: String,
    pub rpc_bigtable_app_profile_id: String,
    pub rpc_bigtable_max_message_size: String,
    pub rpc_max_request_body_size: String,
    pub rpc_pubsub_worker_threads: String,
    pub rpc_pubsub_notification_threads: String,

    pub maximum_local_snapshot_age: String,
    pub maximum_full_snapshot_archives_to_retain: String,
    pub maximum_incremental_snapshot_archives_to_retain: String,
    pub snapshot_packager_niceness_adjustment: String,
    pub full_snapshot_archive_interval_slots: String,
    pub incremental_snapshot_archive_interval_slots: String,
    pub min_snapshot_download_speed: String,
    pub max_snapshot_download_abort: String,

    pub contact_debug_interval: String,

    pub snapshot_version: SnapshotVersion,
    pub snapshot_archive_format: String,

    pub rocksdb_shred_compaction: String,
    pub rocksdb_ledger_compression: String,
    pub rocksdb_perf_sample_interval: String,

    pub accounts_shrink_optimize_total_space: String,
    pub accounts_shrink_ratio: String,
    pub tpu_connection_pool_size: String,
    pub tpu_max_connections_per_ipaddr_per_minute: String,
    pub num_quic_endpoints: String,
    pub vote_use_quic: String,

    // Exit subcommand
    pub exit_min_idle_time: String,
    pub exit_max_delinquent_stake: String,

    // Wait subcommand
    pub wait_for_restart_window_min_idle_time: String,
    pub wait_for_restart_window_max_delinquent_stake: String,

    pub banking_trace_dir_byte_limit: String,

    pub wen_restart_path: String,

    pub thread_args: DefaultThreadArgs,
}

/// 默认参数
impl DefaultArgs {
    pub fn new() -> Self {
        let default_send_transaction_service_config = send_transaction_service::Config::default();

        DefaultArgs {
            bind_address: "0.0.0.0".to_string(),
            ledger_path: "ledger".to_string(),
            dynamic_port_range: format!("{}-{}", VALIDATOR_PORT_RANGE.0, VALIDATOR_PORT_RANGE.1),
            maximum_local_snapshot_age: "2500".to_string(),
            genesis_archive_unpacked_size: MAX_GENESIS_ARCHIVE_UNPACKED_SIZE.to_string(),
            rpc_max_multiple_accounts: MAX_MULTIPLE_ACCOUNTS.to_string(),
            health_check_slot_distance: DELINQUENT_VALIDATOR_SLOT_DISTANCE.to_string(),
            tower_storage: "file".to_string(),
            etcd_domain_name: "localhost".to_string(),
            rpc_pubsub_max_active_subscriptions: PubSubConfig::default()
                .max_active_subscriptions
                .to_string(),
            rpc_pubsub_queue_capacity_items: PubSubConfig::default()
                .queue_capacity_items
                .to_string(),
            rpc_pubsub_queue_capacity_bytes: PubSubConfig::default()
                .queue_capacity_bytes
                .to_string(),
            send_transaction_service_config: send_transaction_service::Config::default(),
            rpc_send_transaction_retry_ms: default_send_transaction_service_config
                .retry_rate_ms
                .to_string(),
            rpc_send_transaction_batch_ms: default_send_transaction_service_config
                .batch_send_rate_ms
                .to_string(),
            rpc_send_transaction_leader_forward_count: default_send_transaction_service_config
                .leader_forward_count
                .to_string(),
            rpc_send_transaction_service_max_retries: default_send_transaction_service_config
                .service_max_retries
                .to_string(),
            rpc_send_transaction_batch_size: default_send_transaction_service_config
                .batch_size
                .to_string(),
            rpc_send_transaction_retry_pool_max_size: default_send_transaction_service_config
                .retry_pool_max_size
                .to_string(),
            rpc_threads: num_cpus::get().to_string(),
            rpc_blocking_threads: 1.max(num_cpus::get() / 4).to_string(),
            rpc_niceness_adjustment: "0".to_string(),
            rpc_bigtable_timeout: "30".to_string(),
            rpc_bigtable_instance_name: solana_storage_bigtable::DEFAULT_INSTANCE_NAME.to_string(),
            rpc_bigtable_app_profile_id: solana_storage_bigtable::DEFAULT_APP_PROFILE_ID
                .to_string(),
            rpc_bigtable_max_message_size: solana_storage_bigtable::DEFAULT_MAX_MESSAGE_SIZE
                .to_string(),
            rpc_pubsub_worker_threads: "4".to_string(),
            rpc_pubsub_notification_threads: get_thread_count().to_string(),
            maximum_full_snapshot_archives_to_retain: DEFAULT_MAX_FULL_SNAPSHOT_ARCHIVES_TO_RETAIN
                .to_string(),
            maximum_incremental_snapshot_archives_to_retain:
                DEFAULT_MAX_INCREMENTAL_SNAPSHOT_ARCHIVES_TO_RETAIN.to_string(),
            snapshot_packager_niceness_adjustment: "0".to_string(),
            full_snapshot_archive_interval_slots: DEFAULT_FULL_SNAPSHOT_ARCHIVE_INTERVAL_SLOTS
                .to_string(),
            incremental_snapshot_archive_interval_slots:
                DEFAULT_INCREMENTAL_SNAPSHOT_ARCHIVE_INTERVAL_SLOTS.to_string(),
            min_snapshot_download_speed: DEFAULT_MIN_SNAPSHOT_DOWNLOAD_SPEED.to_string(),
            max_snapshot_download_abort: MAX_SNAPSHOT_DOWNLOAD_ABORT.to_string(),
            snapshot_archive_format: DEFAULT_ARCHIVE_COMPRESSION.to_string(),
            contact_debug_interval: "120000".to_string(),
            snapshot_version: SnapshotVersion::default(),
            rocksdb_shred_compaction: "level".to_string(),
            rocksdb_ledger_compression: "none".to_string(),
            rocksdb_perf_sample_interval: "0".to_string(),
            accounts_shrink_optimize_total_space: DEFAULT_ACCOUNTS_SHRINK_OPTIMIZE_TOTAL_SPACE
                .to_string(),
            accounts_shrink_ratio: DEFAULT_ACCOUNTS_SHRINK_RATIO.to_string(),
            tpu_connection_pool_size: DEFAULT_TPU_CONNECTION_POOL_SIZE.to_string(),
            tpu_max_connections_per_ipaddr_per_minute:
                DEFAULT_MAX_CONNECTIONS_PER_IPADDR_PER_MINUTE.to_string(),
            vote_use_quic: DEFAULT_VOTE_USE_QUIC.to_string(),
            num_quic_endpoints: DEFAULT_QUIC_ENDPOINTS.to_string(),
            rpc_max_request_body_size: MAX_REQUEST_BODY_SIZE.to_string(),
            exit_min_idle_time: "10".to_string(),
            exit_max_delinquent_stake: "5".to_string(),
            wait_for_restart_window_min_idle_time: "10".to_string(),
            wait_for_restart_window_max_delinquent_stake: "5".to_string(),
            banking_trace_dir_byte_limit: BANKING_TRACE_DIR_DEFAULT_BYTE_LIMIT.to_string(),
            wen_restart_path: "wen_restart_progress.proto".to_string(),
            thread_args: DefaultThreadArgs::default(),
        }
    }
}

impl Default for DefaultArgs {
    /// 默认参数，见模块其他注释
    fn default() -> Self {
        Self::new()
    }
}

/// 检查端口是不是u16
pub fn port_validator(port: String) -> Result<(), String> {
    port.parse::<u16>()
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

/// 检查端口范围
pub fn port_range_validator(port_range: String) -> Result<(), String> {
    if let Some((start, end)) = solana_net_utils::parse_port_range(&port_range) {
        if end - start < MINIMUM_VALIDATOR_PORT_RANGE_WIDTH {
            Err(format!(
                "Port range is too small.  Try --dynamic-port-range {}-{}",
                start,
                start + MINIMUM_VALIDATOR_PORT_RANGE_WIDTH
            ))
        } else if end.checked_add(QUIC_PORT_OFFSET).is_none() {
            Err("Invalid dynamic_port_range.".to_string())
        } else {
            Ok(())
        }
    } else {
        Err("Invalid port range".to_string())
    }
}

/// 检查是否是哈希
fn hash_validator(hash: String) -> Result<(), String> {
    Hash::from_str(&hash)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

/// Test validator
/// 测试版验证器，暂不考虑
pub fn test_app<'a>(version: &'a str, default_args: &'a DefaultTestArgs) -> App<'a, 'a> {
    return App::new("solana-test-validator")
        .about("Test Validator")
        .version(version)
        .arg({
            let arg = Arg::with_name("config_file")
                .short("C")
                .long("config")
                .value_name("PATH")
                .takes_value(true)
                .help("Configuration file to use");
            if let Some(ref config_file) = *solana_cli_config::CONFIG_FILE {
                arg.default_value(config_file)
            } else {
                arg
            }
        })
        .arg(
            Arg::with_name("json_rpc_url")
                .short("u")
                .long("url")
                .value_name("URL_OR_MONIKER")
                .takes_value(true)
                .validator(is_url_or_moniker)
                .help(
                    "URL for Solana's JSON RPC or moniker (or their first letter): \
                     [mainnet-beta, testnet, devnet, localhost]",
                ),
        )
        .arg(
            Arg::with_name("mint_address")
                .long("mint")
                .value_name("PUBKEY")
                .validator(is_pubkey)
                .takes_value(true)
                .help(
                    "Address of the mint account that will receive tokens created at genesis. If \
                     the ledger already exists then this parameter is silently ignored \
                     [default: client keypair]",
                ),
        )
        .arg(
            Arg::with_name("ledger_path")
                .short("l")
                .long("ledger")
                .value_name("DIR")
                .takes_value(true)
                .required(true)
                .default_value("test-ledger")
                .help("Use DIR as ledger location"),
        )
        .arg(
            Arg::with_name("reset")
                .short("r")
                .long("reset")
                .takes_value(false)
                .help(
                    "Reset the ledger to genesis if it exists. By default the validator will \
                     resume an existing ledger (if present)",
                ),
        )
        .arg(
            Arg::with_name("quiet")
                .short("q")
                .long("quiet")
                .takes_value(false)
                .conflicts_with("log")
                .help("Quiet mode: suppress normal output"),
        )
        .arg(
            Arg::with_name("log")
                .long("log")
                .takes_value(false)
                .conflicts_with("quiet")
                .help("Log mode: stream the validator log"),
        )
        .arg(
            Arg::with_name("account_indexes")
                .long("account-index")
                .takes_value(true)
                .multiple(true)
                .possible_values(&["program-id", "spl-token-owner", "spl-token-mint"])
                .value_name("INDEX")
                .help("Enable an accounts index, indexed by the selected account field"),
        )
        .arg(
            Arg::with_name("faucet_port")
                .long("faucet-port")
                .value_name("PORT")
                .takes_value(true)
                .default_value(&default_args.faucet_port)
                .validator(port_validator)
                .help("Enable the faucet on this port"),
        )
        .arg(
            Arg::with_name("rpc_port")
                .long("rpc-port")
                .value_name("PORT")
                .takes_value(true)
                .default_value(&default_args.rpc_port)
                .validator(port_validator)
                .help("Enable JSON RPC on this port, and the next port for the RPC websocket"),
        )
        .arg(
            Arg::with_name("enable_rpc_bigtable_ledger_storage")
                .long("enable-rpc-bigtable-ledger-storage")
                .takes_value(false)
                .hidden(hidden_unless_forced())
                .help(
                    "Fetch historical transaction info from a BigTable instance as a fallback to \
                     local ledger data",
                ),
        )
        .arg(
            Arg::with_name("enable_bigtable_ledger_upload")
                .long("enable-bigtable-ledger-upload")
                .takes_value(false)
                .hidden(hidden_unless_forced())
                .help("Upload new confirmed blocks into a BigTable instance"),
        )
        .arg(
            Arg::with_name("rpc_bigtable_instance")
                .long("rpc-bigtable-instance")
                .value_name("INSTANCE_NAME")
                .takes_value(true)
                .hidden(hidden_unless_forced())
                .default_value("solana-ledger")
                .help("Name of BigTable instance to target"),
        )
        .arg(
            Arg::with_name("rpc_bigtable_app_profile_id")
                .long("rpc-bigtable-app-profile-id")
                .value_name("APP_PROFILE_ID")
                .takes_value(true)
                .hidden(hidden_unless_forced())
                .default_value(solana_storage_bigtable::DEFAULT_APP_PROFILE_ID)
                .help("Application profile id to use in Bigtable requests"),
        )
        .arg(
            Arg::with_name("rpc_pubsub_enable_vote_subscription")
                .long("rpc-pubsub-enable-vote-subscription")
                .takes_value(false)
                .help("Enable the unstable RPC PubSub `voteSubscribe` subscription"),
        )
        .arg(
            Arg::with_name("rpc_pubsub_enable_block_subscription")
                .long("rpc-pubsub-enable-block-subscription")
                .takes_value(false)
                .help("Enable the unstable RPC PubSub `blockSubscribe` subscription"),
        )
        .arg(
            Arg::with_name("bpf_program")
                .long("bpf-program")
                .value_names(&["ADDRESS_OR_KEYPAIR", "SBF_PROGRAM.SO"])
                .takes_value(true)
                .number_of_values(2)
                .multiple(true)
                .help(
                    "Add a SBF program to the genesis configuration with upgrades disabled. If \
                     the ledger already exists then this parameter is silently ignored. The first \
                     argument can be a pubkey string or path to a keypair",
                ),
        )
        .arg(
            Arg::with_name("upgradeable_program")
                .long("upgradeable-program")
                .value_names(&["ADDRESS_OR_KEYPAIR", "SBF_PROGRAM.SO", "UPGRADE_AUTHORITY"])
                .takes_value(true)
                .number_of_values(3)
                .multiple(true)
                .help(
                    "Add an upgradeable SBF program to the genesis configuration. If the ledger \
                     already exists then this parameter is silently ignored. First and third \
                     arguments can be a pubkey string or path to a keypair. Upgrade authority set \
                     to \"none\" disables upgrades",
                ),
        )
        .arg(
            Arg::with_name("account")
                .long("account")
                .value_names(&["ADDRESS", "DUMP.JSON"])
                .takes_value(true)
                .number_of_values(2)
                .allow_hyphen_values(true)
                .multiple(true)
                .help(
                    "Load an account from the provided JSON file (see `solana account --help` on \
                     how to dump an account to file). Files are searched for relatively to CWD \
                     and tests/fixtures. If ADDRESS is omitted via the `-` placeholder, the one \
                     in the file will be used. If the ledger already exists then this parameter \
                     is silently ignored",
                ),
        )
        .arg(
            Arg::with_name("account_dir")
                .long("account-dir")
                .value_name("DIRECTORY")
                .validator(|value| {
                    value
                        .parse::<PathBuf>()
                        .map_err(|err| format!("error parsing '{value}': {err}"))
                        .and_then(|path| {
                            if path.exists() && path.is_dir() {
                                Ok(())
                            } else {
                                Err(format!(
                                    "path does not exist or is not a directory: {value}"
                                ))
                            }
                        })
                })
                .takes_value(true)
                .multiple(true)
                .help(
                    "Load all the accounts from the JSON files found in the specified DIRECTORY \
                     (see also the `--account` flag). If the ledger already exists then this \
                     parameter is silently ignored",
                ),
        )
        .arg(
            Arg::with_name("ticks_per_slot")
                .long("ticks-per-slot")
                .value_name("TICKS")
                .validator(|value| {
                    value
                        .parse::<u64>()
                        .map_err(|err| format!("error parsing '{value}': {err}"))
                        .and_then(|ticks| {
                            if ticks < MINIMUM_TICKS_PER_SLOT {
                                Err(format!("value must be >= {MINIMUM_TICKS_PER_SLOT}"))
                            } else {
                                Ok(())
                            }
                        })
                })
                .takes_value(true)
                .help("The number of ticks in a slot"),
        )
        .arg(
            Arg::with_name("slots_per_epoch")
                .long("slots-per-epoch")
                .value_name("SLOTS")
                .validator(|value| {
                    value
                        .parse::<Slot>()
                        .map_err(|err| format!("error parsing '{value}': {err}"))
                        .and_then(|slot| {
                            if slot < MINIMUM_SLOTS_PER_EPOCH {
                                Err(format!("value must be >= {MINIMUM_SLOTS_PER_EPOCH}"))
                            } else {
                                Ok(())
                            }
                        })
                })
                .takes_value(true)
                .help(
                    "Override the number of slots in an epoch. If the ledger already exists then \
                     this parameter is silently ignored",
                ),
        )
        .arg(
            Arg::with_name("gossip_port")
                .long("gossip-port")
                .value_name("PORT")
                .takes_value(true)
                .help("Gossip port number for the validator"),
        )
        .arg(
            Arg::with_name("gossip_host")
                .long("gossip-host")
                .value_name("HOST")
                .takes_value(true)
                .validator(solana_net_utils::is_host)
                .help(
                    "Gossip DNS name or IP address for the validator to advertise in gossip \
                     [default: 127.0.0.1]",
                ),
        )
        .arg(
            Arg::with_name("dynamic_port_range")
                .long("dynamic-port-range")
                .value_name("MIN_PORT-MAX_PORT")
                .takes_value(true)
                .validator(port_range_validator)
                .help("Range to use for dynamically assigned ports [default: 1024-65535]"),
        )
        .arg(
            Arg::with_name("bind_address")
                .long("bind-address")
                .value_name("HOST")
                .takes_value(true)
                .validator(solana_net_utils::is_host)
                .default_value("0.0.0.0")
                .help("IP address to bind the validator ports [default: 0.0.0.0]"),
        )
        .arg(
            Arg::with_name("clone_account")
                .long("clone")
                .short("c")
                .value_name("ADDRESS")
                .takes_value(true)
                .validator(is_pubkey_or_keypair)
                .multiple(true)
                .requires("json_rpc_url")
                .help(
                    "Copy an account from the cluster referenced by the --url argument the \
                     genesis configuration. If the ledger already exists then this parameter is \
                     silently ignored",
                ),
        )
        .arg(
            Arg::with_name("maybe_clone_account")
                .long("maybe-clone")
                .value_name("ADDRESS")
                .takes_value(true)
                .validator(is_pubkey_or_keypair)
                .multiple(true)
                .requires("json_rpc_url")
                .help(
                    "Copy an account from the cluster referenced by the --url argument, skipping \
                     it if it doesn't exist. If the ledger already exists then this parameter is \
                     silently ignored",
                ),
        )
        .arg(
            Arg::with_name("clone_upgradeable_program")
                .long("clone-upgradeable-program")
                .value_name("ADDRESS")
                .takes_value(true)
                .validator(is_pubkey_or_keypair)
                .multiple(true)
                .requires("json_rpc_url")
                .help(
                    "Copy an upgradeable program and its executable data from the cluster \
                     referenced by the --url argument the genesis configuration. If the ledger \
                     already exists then this parameter is silently ignored",
                ),
        )
        .arg(
            Arg::with_name("warp_slot")
                .required(false)
                .long("warp-slot")
                .short("w")
                .takes_value(true)
                .value_name("WARP_SLOT")
                .validator(is_slot)
                .min_values(0)
                .max_values(1)
                .help(
                    "Warp the ledger to WARP_SLOT after starting the validator. If no slot is \
                     provided then the current slot of the cluster referenced by the --url \
                     argument will be used",
                ),
        )
        .arg(
            Arg::with_name("limit_ledger_size")
                .long("limit-ledger-size")
                .value_name("SHRED_COUNT")
                .takes_value(true)
                .default_value(default_args.limit_ledger_size.as_str())
                .help("Keep this amount of shreds in root slots."),
        )
        .arg(
            Arg::with_name("faucet_sol")
                .long("faucet-sol")
                .takes_value(true)
                .value_name("SOL")
                .default_value(default_args.faucet_sol.as_str())
                .help(
                    "Give the faucet address this much SOL in genesis. If the ledger already \
                     exists then this parameter is silently ignored",
                ),
        )
        .arg(
            Arg::with_name("faucet_time_slice_secs")
                .long("faucet-time-slice-secs")
                .takes_value(true)
                .value_name("SECS")
                .default_value(default_args.faucet_time_slice_secs.as_str())
                .help("Time slice (in secs) over which to limit faucet requests"),
        )
        .arg(
            Arg::with_name("faucet_per_time_sol_cap")
                .long("faucet-per-time-sol-cap")
                .takes_value(true)
                .value_name("SOL")
                .min_values(0)
                .max_values(1)
                .help("Per-time slice limit for faucet requests, in SOL"),
        )
        .arg(
            Arg::with_name("faucet_per_request_sol_cap")
                .long("faucet-per-request-sol-cap")
                .takes_value(true)
                .value_name("SOL")
                .min_values(0)
                .max_values(1)
                .help("Per-request limit for faucet requests, in SOL"),
        )
        .arg(
            Arg::with_name("geyser_plugin_config")
                .long("geyser-plugin-config")
                .alias("accountsdb-plugin-config")
                .value_name("FILE")
                .takes_value(true)
                .multiple(true)
                .help("Specify the configuration file for the Geyser plugin."),
        )
        .arg(
            Arg::with_name("deactivate_feature")
                .long("deactivate-feature")
                .takes_value(true)
                .value_name("FEATURE_PUBKEY")
                .validator(is_pubkey)
                .multiple(true)
                .help("deactivate this feature in genesis."),
        )
        .arg(
            Arg::with_name("compute_unit_limit")
                .long("compute-unit-limit")
                .alias("max-compute-units")
                .value_name("COMPUTE_UNITS")
                .validator(is_parsable::<u64>)
                .takes_value(true)
                .help("Override the runtime's compute unit limit per transaction"),
        )
        .arg(
            Arg::with_name("log_messages_bytes_limit")
                .long("log-messages-bytes-limit")
                .value_name("BYTES")
                .validator(is_parsable::<usize>)
                .takes_value(true)
                .help("Maximum number of bytes written to the program log before truncation"),
        )
        .arg(
            Arg::with_name("transaction_account_lock_limit")
                .long("transaction-account-lock-limit")
                .value_name("NUM_ACCOUNTS")
                .validator(is_parsable::<u64>)
                .takes_value(true)
                .help("Override the runtime's account lock limit per transaction"),
        )
        .arg(
            Arg::with_name("clone_feature_set")
                .long("clone-feature-set")
                .takes_value(false)
                .requires("json_rpc_url")
                .help(
                    "Copy a feature set from the cluster referenced by the --url \
                     argument in the genesis configuration. If the ledger \
                     already exists then this parameter is silently ignored",
                ),
        );
}

/// 测试验证器默认参数，不考虑
pub struct DefaultTestArgs {
    pub rpc_port: String,
    pub faucet_port: String,
    pub limit_ledger_size: String,
    pub faucet_sol: String,
    pub faucet_time_slice_secs: String,
}

impl DefaultTestArgs {
    pub fn new() -> Self {
        DefaultTestArgs {
            rpc_port: rpc_port::DEFAULT_RPC_PORT.to_string(),
            faucet_port: FAUCET_PORT.to_string(),
            /* 10,000 was derived empirically by watching the size
             * of the rocksdb/ directory self-limit itself to the
             * 40MB-150MB range when running `solana-test-validator`
             */
            limit_ledger_size: 10_000.to_string(),
            faucet_sol: (1_000_000.).to_string(),
            faucet_time_slice_secs: (faucet::TIME_SLICE).to_string(),
        }
    }
}

impl Default for DefaultTestArgs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn make_sure_deprecated_arguments_are_sorted_alphabetically() {
        let deprecated = deprecated_arguments();

        for i in 0..deprecated.len().saturating_sub(1) {
            let curr_name = deprecated[i].arg.b.name;
            let next_name = deprecated[i + 1].arg.b.name;

            assert!(
                curr_name != next_name,
                "Arguments in `deprecated_arguments()` should be distinct.\nArguments {} and {} \
                 use the same name: {}",
                i,
                i + 1,
                curr_name,
            );

            assert!(
                curr_name < next_name,
                "To generate better diffs and for readability purposes, `deprecated_arguments()` \
                 should list arguments in alphabetical order.\nArguments {} and {} are \
                 not.\nArgument {} name: {}\nArgument {} name: {}",
                i,
                i + 1,
                i,
                curr_name,
                i + 1,
                next_name,
            );
        }
    }
}
