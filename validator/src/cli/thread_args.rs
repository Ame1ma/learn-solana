//! Arguments for controlling the number of threads allocated for various tasks

use {
    clap::{value_t_or_exit, Arg, ArgMatches},
    solana_accounts_db::{accounts_db, accounts_index},
    solana_clap_utils::{hidden_unless_forced, input_validators::is_within_range},
    solana_rayon_threadlimit::{get_max_thread_count, get_thread_count},
    std::{num::NonZeroUsize, ops::RangeInclusive},
};

// Need this struct to provide &str whose lifetime matches that of the CLAP Arg's
/// 默认线程数配置，含义见模块中的其他注释
pub struct DefaultThreadArgs {
    pub accounts_db_clean_threads: String,
    pub accounts_db_foreground_threads: String,
    pub accounts_db_hash_threads: String,
    pub accounts_index_flush_threads: String,
    pub ip_echo_server_threads: String,
    pub rayon_global_threads: String,
    pub replay_forks_threads: String,
    pub replay_transactions_threads: String,
    pub rocksdb_compaction_threads: String,
    pub rocksdb_flush_threads: String,
    pub tvu_receive_threads: String,
    pub tvu_sigverify_threads: String,
}

impl Default for DefaultThreadArgs {
    fn default() -> Self {
        Self {
            accounts_db_clean_threads: AccountsDbCleanThreadsArg::bounded_default().to_string(),
            accounts_db_foreground_threads: AccountsDbForegroundThreadsArg::bounded_default()
                .to_string(),
            accounts_db_hash_threads: AccountsDbHashThreadsArg::bounded_default().to_string(),
            accounts_index_flush_threads: AccountsIndexFlushThreadsArg::bounded_default()
                .to_string(),
            ip_echo_server_threads: IpEchoServerThreadsArg::bounded_default().to_string(),
            rayon_global_threads: RayonGlobalThreadsArg::bounded_default().to_string(),
            replay_forks_threads: ReplayForksThreadsArg::bounded_default().to_string(),
            replay_transactions_threads: ReplayTransactionsThreadsArg::bounded_default()
                .to_string(),
            rocksdb_compaction_threads: RocksdbCompactionThreadsArg::bounded_default().to_string(),
            rocksdb_flush_threads: RocksdbFlushThreadsArg::bounded_default().to_string(),
            tvu_receive_threads: TvuReceiveThreadsArg::bounded_default().to_string(),
            tvu_sigverify_threads: TvuShredSigverifyThreadsArg::bounded_default().to_string(),
        }
    }
}

/// 线程配置
pub fn thread_args<'a>(defaults: &DefaultThreadArgs) -> Vec<Arg<'_, 'a>> {
    vec![
        /// 见类型注释
        new_thread_arg::<AccountsDbCleanThreadsArg>(&defaults.accounts_db_clean_threads),
        new_thread_arg::<AccountsDbForegroundThreadsArg>(&defaults.accounts_db_foreground_threads),
        new_thread_arg::<AccountsDbHashThreadsArg>(&defaults.accounts_db_hash_threads),
        new_thread_arg::<AccountsIndexFlushThreadsArg>(&defaults.accounts_index_flush_threads),
        new_thread_arg::<IpEchoServerThreadsArg>(&defaults.ip_echo_server_threads),
        new_thread_arg::<RayonGlobalThreadsArg>(&defaults.rayon_global_threads),
        new_thread_arg::<ReplayForksThreadsArg>(&defaults.replay_forks_threads),
        new_thread_arg::<ReplayTransactionsThreadsArg>(&defaults.replay_transactions_threads),
        new_thread_arg::<RocksdbCompactionThreadsArg>(&defaults.rocksdb_compaction_threads),
        new_thread_arg::<RocksdbFlushThreadsArg>(&defaults.rocksdb_flush_threads),
        new_thread_arg::<TvuReceiveThreadsArg>(&defaults.tvu_receive_threads),
        new_thread_arg::<TvuShredSigverifyThreadsArg>(&defaults.tvu_sigverify_threads),
    ]
}

/// 有意思的写法，写了一个 trait，然后为这个 trait 写了产生 Arg 项的方法
fn new_thread_arg<'a, T: ThreadArg>(default: &str) -> Arg<'_, 'a> {
    Arg::with_name(T::NAME)
        .long(T::LONG_NAME)
        .takes_value(true)
        .value_name("NUMBER")
        .default_value(default)
        .validator(|num| is_within_range(num, T::range()))
        .hidden(hidden_unless_forced())
        .help(T::HELP)
}

/// 线程数配置，含义见模块中的其他注释
pub struct NumThreadConfig {
    pub accounts_db_clean_threads: NonZeroUsize,
    pub accounts_db_foreground_threads: NonZeroUsize,
    pub accounts_db_hash_threads: NonZeroUsize,
    pub accounts_index_flush_threads: NonZeroUsize,
    pub ip_echo_server_threads: NonZeroUsize,
    pub rayon_global_threads: NonZeroUsize,
    pub replay_forks_threads: NonZeroUsize,
    pub replay_transactions_threads: NonZeroUsize,
    pub rocksdb_compaction_threads: NonZeroUsize,
    pub rocksdb_flush_threads: NonZeroUsize,
    pub tvu_receive_threads: NonZeroUsize,
    pub tvu_sigverify_threads: NonZeroUsize,
}

/// 解析命令行参数到线程数配置
pub fn parse_num_threads_args(matches: &ArgMatches) -> NumThreadConfig {
    NumThreadConfig {
        accounts_db_clean_threads: value_t_or_exit!(
            matches,
            AccountsDbCleanThreadsArg::NAME,
            NonZeroUsize
        ),
        accounts_db_foreground_threads: value_t_or_exit!(
            matches,
            AccountsDbForegroundThreadsArg::NAME,
            NonZeroUsize
        ),
        accounts_db_hash_threads: value_t_or_exit!(
            matches,
            AccountsDbHashThreadsArg::NAME,
            NonZeroUsize
        ),
        accounts_index_flush_threads: value_t_or_exit!(
            matches,
            AccountsIndexFlushThreadsArg::NAME,
            NonZeroUsize
        ),
        ip_echo_server_threads: value_t_or_exit!(
            matches,
            IpEchoServerThreadsArg::NAME,
            NonZeroUsize
        ),
        rayon_global_threads: value_t_or_exit!(matches, RayonGlobalThreadsArg::NAME, NonZeroUsize),
        replay_forks_threads: if matches.is_present("replay_slots_concurrently") {
            NonZeroUsize::new(4).expect("4 is non-zero")
        } else {
            value_t_or_exit!(matches, ReplayForksThreadsArg::NAME, NonZeroUsize)
        },
        replay_transactions_threads: value_t_or_exit!(
            matches,
            ReplayTransactionsThreadsArg::NAME,
            NonZeroUsize
        ),
        rocksdb_compaction_threads: value_t_or_exit!(
            matches,
            RocksdbCompactionThreadsArg::NAME,
            NonZeroUsize
        ),
        rocksdb_flush_threads: value_t_or_exit!(
            matches,
            RocksdbFlushThreadsArg::NAME,
            NonZeroUsize
        ),
        tvu_receive_threads: value_t_or_exit!(matches, TvuReceiveThreadsArg::NAME, NonZeroUsize),
        tvu_sigverify_threads: value_t_or_exit!(
            matches,
            TvuShredSigverifyThreadsArg::NAME,
            NonZeroUsize
        ),
    }
}

/// Configuration for CLAP arguments that control the number of threads for various functions
/// 线程数配置 trait，由 new_thread_arg 来转换成 clap 的 Arg
trait ThreadArg {
    /// The argument's name
    /// 参数名
    const NAME: &'static str;
    /// The argument's long name
    /// 完整参数名
    const LONG_NAME: &'static str;
    /// The argument's help message
    /// 命令帮助
    const HELP: &'static str;

    /// The default number of threads
    /// 默认线程数
    fn default() -> usize;
    /// The default number of threads, bounded by Self::max()
    /// This prevents potential CLAP issues on low core count machines where
    /// a fixed value in Self::default() could be greater than Self::max()
    /// 限制默认线程数不超量
    fn bounded_default() -> usize {
        std::cmp::min(Self::default(), Self::max())
    }
    /// The minimum allowed number of threads (inclusive)
    /// 最小值
    fn min() -> usize {
        1
    }
    /// The maximum allowed number of threads (inclusive)
    /// 最大值
    fn max() -> usize {
        // By default, no thread pool should scale over the number of the machine's threads
        get_max_thread_count()
    }
    /// The range of allowed number of threads (inclusive on both ends)
    /// 范围
    fn range() -> RangeInclusive<usize> {
        RangeInclusive::new(Self::min(), Self::max())
    }
}

/// 账户数据库清理线程数，默认为核数的 1/4
struct AccountsDbCleanThreadsArg;
impl ThreadArg for AccountsDbCleanThreadsArg {
    const NAME: &'static str = "accounts_db_clean_threads";
    const LONG_NAME: &'static str = "accounts-db-clean-threads";
    const HELP: &'static str = "Number of threads to use for cleaning AccountsDb";

    fn default() -> usize {
        accounts_db::quarter_thread_count()
    }
}

/// 帐户数据库（accounts-db） 在执行时使用的前台线程数。帐户数据库是 Solana 存储和访问帐户数据的核心部分，
/// 它对 Solana 节点的性能和响应速度至关重要，尤其是在交易量高、帐户数量多的情况下。
/// 默认核数 1/2
struct AccountsDbForegroundThreadsArg;
impl ThreadArg for AccountsDbForegroundThreadsArg {
    const NAME: &'static str = "accounts_db_foreground_threads";
    const LONG_NAME: &'static str = "accounts-db-foreground-threads";
    const HELP: &'static str = "Number of threads to use for AccountsDb block processing";

    fn default() -> usize {
        accounts_db::default_num_foreground_threads()
    }
}

/// 帐户数据库（accounts-db） 在执行时使用的后台线程数，进行哈希计算。
/// 默认核数的 1/8
struct AccountsDbHashThreadsArg;
impl ThreadArg for AccountsDbHashThreadsArg {
    const NAME: &'static str = "accounts_db_hash_threads";
    const LONG_NAME: &'static str = "accounts-db-hash-threads";
    const HELP: &'static str = "Number of threads to use for background accounts hashing";

    fn default() -> usize {
        accounts_db::default_num_hash_threads().get()
    }
}

/// 用于刷新账户索引的线程数
/// 默认核数的 1/4
struct AccountsIndexFlushThreadsArg;
impl ThreadArg for AccountsIndexFlushThreadsArg {
    const NAME: &'static str = "accounts_index_flush_threads";
    const LONG_NAME: &'static str = "accounts-index-flush-threads";
    const HELP: &'static str = "Number of threads to use for flushing the accounts index";

    fn default() -> usize {
        accounts_index::default_num_flush_threads().get()
    }
}

/// 用于 ip echo 的线程数，默认 1/2 核数
/// IP Echo 服务器 是 Solana 节点中的一个网络功能，主要用于接收并响应来自其他节点或客户端的简单网络请求（通常是 ICMP Echo 请求，也就是常见的 "ping" 请求）。
/// 这个功能的目的是帮助验证和测试节点的网络连通性，尤其是在网络拓扑测试和故障排查时。它类似于传统的 ping 命令，可以验证某个节点是否能够通过网络访问。
struct IpEchoServerThreadsArg;
impl ThreadArg for IpEchoServerThreadsArg {
    const NAME: &'static str = "ip_echo_server_threads";
    const LONG_NAME: &'static str = "ip-echo-server-threads";
    const HELP: &'static str = "Number of threads to use for the IP echo server";

    fn default() -> usize {
        solana_net_utils::DEFAULT_IP_ECHO_SERVER_THREADS.get()
    }
    fn min() -> usize {
        solana_net_utils::MINIMUM_IP_ECHO_SERVER_THREADS.get()
    }
}

/// 全局 rayon 线程数，默认 1/1 核数
struct RayonGlobalThreadsArg;
impl ThreadArg for RayonGlobalThreadsArg {
    const NAME: &'static str = "rayon_global_threads";
    const LONG_NAME: &'static str = "rayon-global-threads";
    const HELP: &'static str = "Number of threads to use for the global rayon thread pool";

    fn default() -> usize {
        get_max_thread_count()
    }
}

/// 用于在不同分叉上重放块的线程数， 默认 1 线程
struct ReplayForksThreadsArg;
impl ThreadArg for ReplayForksThreadsArg {
    const NAME: &'static str = "replay_forks_threads";
    const LONG_NAME: &'static str = "replay-forks-threads";
    const HELP: &'static str = "Number of threads to use for replay of blocks on different forks";

    fn default() -> usize {
        // Default to single threaded fork execution
        1
    }
    fn max() -> usize {
        // Choose a value that is small enough to limit the overhead of having a large thread pool
        // while also being large enough to allow replay of all active forks in most scenarios
        4
    }
}

/// 重放交易的线程数， 默认 1/1 核数
struct ReplayTransactionsThreadsArg;
impl ThreadArg for ReplayTransactionsThreadsArg {
    const NAME: &'static str = "replay_transactions_threads";
    const LONG_NAME: &'static str = "replay-transactions-threads";
    const HELP: &'static str = "Number of threads to use for transaction replay";

    fn default() -> usize {
        get_max_thread_count()
    }
}

// 在 Solana 中，**RocksDB** 是主要的持久化存储引擎，用于高效存储和检索关键数据，特别是与帐户（account）相关的信息。除了 RocksDB，Solana 还使用了其他持久化方案来处理不同类型的数据和优化系统的性能。下面我将详细解释每种方案及其在 Solana 中的作用。
// ### 1. **RocksDB 的作用：**
// RocksDB 是 Solana 的主要持久化存储引擎，负责存储和管理所有的帐户数据、交易历史等信息。它是一个高性能的键值数据库（Key-Value Store），并且非常适合处理大量的写操作和大规模的数据集。
// #### **RocksDB 在 Solana 中的应用：**
// - **帐户数据库（Accounts Database）**：RocksDB 存储了所有帐户的状态，包括帐户的余额、存储的数据和执行的操作等。每个帐户的状态都与其公钥（或地址）关联，并存储在 RocksDB 中。Solana 会定期将这些数据持久化到磁盘中，以保证节点在重启后能够恢复到之前的状态。
// - **银行状态（Bank State）**：Solana 的银行状态也依赖 RocksDB 进行存储，银行状态包括帐户的当前状态和其他全局状态。
// - **交易历史**：RocksDB 还用于存储交易历史、区块链数据（如区块头、区块内容等）和其他与交易相关的数据。
// 由于 RocksDB 是一种基于日志的存储引擎，它能够高效地处理大量的写入操作，因此非常适合 Solana 高吞吐量和低延迟的需求。
// ### 2. **Solana 其他持久化方案：**
// 除了 RocksDB，Solana 还使用了其他一些持久化存储方案，分别用于不同的数据类型和组件。主要包括：
// #### **1. 存储日志（WAL, Write-Ahead Log）**
// - **用途**：Solana 使用 WAL（Write-Ahead Log）来确保在数据持久化前记录所有修改操作。WAL 主要用于记录银行状态（Bank State）在执行交易过程中的变化。
// - **实现**：WAL 使得 Solana 能够在节点崩溃或重启时恢复最近的银行状态变更。它确保了交易的一致性，避免数据丢失。对于每个交易，都会在日志中写入其变更记录，直到数据被完全持久化。
// - **组件**：WAL 主要用于 Solana 的账户数据库和银行状态的持久化。
// #### **2. Snapshot（快照）**
// - **用途**：为了降低数据库（尤其是 RocksDB）的访问成本，Solana 定期生成系统的快照。这些快照保存了账本状态和其他关键数据，可以在系统崩溃时恢复。快照提供了一个快速恢复的机制，避免每次启动时都需要重新扫描整个数据库。
// - **实现**：快照主要存储了 **银行状态** 和 **帐户数据库的快照**。通过快照，Solana 可以快速恢复到某个特定时间点的状态，而无需重新加载所有历史数据。
// - **组件**：快照机制涉及到整个 Solana 系统，尤其是在系统重启或灾难恢复时使用。
// #### **3. BPF（Berkeley Packet Filter）程序存储**
// - **用途**：Solana 使用 BPF（Berkeley Packet Filter）程序来执行智能合约（称为程序）。这些程序通常不直接存储在 RocksDB 中，而是通过其他持久化方案管理。Solana 使用类似文件系统的存储方案来管理这些程序的生命周期。
// - **实现**：Solana 会将编译后的 BPF 程序存储在 **程序存储区（Program Storage）** 中，程序数据保存在文件系统中。程序的状态和与智能合约相关的数据被保存在 RocksDB 或其他数据库中。
// - **组件**：智能合约程序和其相关的状态数据。
// #### **4. Ledger（账本）存储**
// - **用途**：Solana 还维护一个完整的账本，其中存储了所有历史区块的详细数据。账本数据是 Solana 区块链的一部分，确保了区块链的完整性和可追溯性。
// - **实现**：账本数据通常以区块的形式保存在文件系统中，且会通过验证者节点的 RocksDB 存储进行管理。账本记录了每个区块的详细信息，包括交易数据、区块头等。
// - **组件**：账本的持久化存储与 RocksDB 和文件系统一起使用，确保交易的历史记录是持久且完整的。
// ### 3. **总结：**
// Solana 使用了多种持久化方案来处理不同类型的数据，这些方案帮助 Solana 实现高效的数据存储、访问和恢复，确保在高吞吐量的情况下系统仍然能够保证数据的一致性和持久性。
// - **RocksDB**：主要用于存储帐户数据、银行状态、交易历史等。
// - **WAL（Write-Ahead Log）**：用于确保交易的一致性，特别是在节点崩溃或重启时恢复银行状态。
// - **Snapshot**：用于快速恢复系统状态，减少对 RocksDB 的访问。
// - **BPF 程序存储**：存储智能合约程序（BPF 程序），并管理其生命周期。
// - **Ledger 存储**：用于持久化区块链的账本数据，确保区块链的完整性和可追溯性。
// 通过这些持久化方案，Solana 能够高效地管理海量的帐户数据、交易记录，并在发生故障时快速恢复，确保系统的稳定性和高效性。

// 是的，Solana 使用 **Append Vecs** 存储账户数据，而 **RocksDB** 主要用于存储和管理与账户相关的索引信息、元数据以及其他一些全局状态数据。虽然两者都涉及账户数据的存储，但它们有不同的职责和设计目标。
// ### 1. **Append Vecs 存储账户数据**
// **Append Vecs** 是 Solana 自定义的一种存储格式，专门用于高效地存储和管理帐户数据。每个帐户在 Solana 中都有一个存储区域，这些存储区域被称为 **Append Vecs**。其主要特点是 **追加写入**（append-only）特性，这使得它在高写入负载下表现得非常高效。下面是一些关于 Append Vecs 的特点和用途：
// #### **Append Vecs 的工作原理：**
// - **追加写入**：当有新的帐户数据需要存储时，Solana 会将数据追加到一个 **Append Vec** 文件中。这种设计的优势在于对数据的写入是顺序的，因此能够最大化磁盘写入的效率。
// - **分割和索引**：Append Vec 是按逻辑块划分的，每个块大小通常为几个 MB。在这些块中，每个帐户的具体数据会被存储。每个块内部是连续的、顺序的文件结构，因此查找和追加操作非常高效。
// - **高效的读取和更新**：Solana 在查询或更新帐户状态时，会快速定位到相应的 **Append Vec**，因为每个帐户的数据是按顺序写入的，可以通过索引快速查找到所需数据。
// - **写入优化**：由于 Append Vec 是追加型存储，因此它减少了磁盘上的随机写入操作，提高了性能，特别是在大量帐户和高频率交易的情况下。
// #### **Append Vec 的存储内容：**
// - **账户数据**：每个帐户的状态（如余额、所有者、数据等）会被存储在 Append Vec 中。
// - **状态更新**：由于 Solana 的账户状态可能会发生变化（如转账、授权等），这些变化通常会追加到新的 **Append Vec** 或覆盖已有的数据块。
// ### 2. **RocksDB 的角色**
// **RocksDB** 是 Solana 的主要持久化存储引擎，它与 Append Vecs 结合使用，负责管理和存储与账户相关的 **索引信息** 和 **元数据**。尽管 **Append Vecs** 存储了账户的实际数据，RocksDB 则用于存储这些数据的索引和一些全局状态信息。具体来说，RocksDB 用来存储以下内容：
// - **账户索引**：RocksDB 存储了账户的索引信息（如账户的公钥与账户数据在 Append Vec 中的位置）。通过这些索引，Solana 可以快速定位到某个账户的数据块，并进行读取或更新操作。
// - **Bank 状态**：RocksDB 存储有关 Solana 网络状态的信息，例如当前区块链状态、账本的元数据、确认的区块链头等。
// - **存储管理**：RocksDB 还用于存储一些与 **Append Vec** 相关的元数据，帮助 Solana 管理存储空间和处理数据的持久化。
// ### 3. **Append Vec 和 RocksDB 之间的关系**
// 尽管 **Append Vecs** 和 **RocksDB** 各自负责不同的存储任务，但它们在 Solana 中是紧密协作的：
// - **RocksDB 用于管理索引和元数据**：Solana 使用 RocksDB 来存储帐户的索引（例如，公钥与账户数据位置的映射），这些索引可以帮助快速定位和读取 **Append Vecs** 中的帐户数据。
// - **Append Vecs 用于高效存储帐户数据**：帐户的实际数据（如余额、存储的数据等）存储在 Append Vec 文件中，而 RocksDB 负责维护与这些数据相关的索引和元数据。
// - **索引和数据分离存储**：Append Vec 存储的是帐户数据的实际内容，而 RocksDB 存储的是这些数据的索引信息。通过这种方式，Solana 能够优化存储访问：读取数据时首先通过 RocksDB 查找索引，然后通过索引定位到正确的 **Append Vec**，从而高效访问帐户数据。
// ### 4. **总结**
// - **Append Vecs** 是 Solana 用于存储账户数据的核心存储格式，它采用顺序追加的方式，优化了写入性能，特别是在处理高频交易时。
// - **RocksDB** 则用于存储和管理索引信息和元数据，它帮助 Solana 快速查找和访问存储在 Append Vec 中的帐户数据。
// - 二者相辅相成：Append Vec 提供了高效的存储方式，而 RocksDB 提供了强大的索引和元数据管理功能，确保 Solana 在高吞吐量和高并发的情况下能够高效地管理和访问帐户数据。
// 这样，Solana 通过结合 **Append Vecs** 和 **RocksDB** 的方式，优化了数据的存储和检索过程，保证了系统的高性能和高可扩展性。

/// rocksdb 压缩线程数， 默认 1/1 核数
struct RocksdbCompactionThreadsArg;
impl ThreadArg for RocksdbCompactionThreadsArg {
    const NAME: &'static str = "rocksdb_compaction_threads";
    const LONG_NAME: &'static str = "rocksdb-compaction-threads";
    const HELP: &'static str = "Number of threads to use for rocksdb (Blockstore) compactions";

    fn default() -> usize {
        solana_ledger::blockstore::default_num_compaction_threads().get()
    }
}

/// rocksdb刷新线程数，默认 1/4 核数
struct RocksdbFlushThreadsArg;
impl ThreadArg for RocksdbFlushThreadsArg {
    const NAME: &'static str = "rocksdb_flush_threads";
    const LONG_NAME: &'static str = "rocksdb-flush-threads";
    const HELP: &'static str = "Number of threads to use for rocksdb (Blockstore) memtable flushes";

    fn default() -> usize {
        solana_ledger::blockstore::default_num_flush_threads().get()
    }
}

/// TVU 线程数， 默认 8 个
struct TvuReceiveThreadsArg;
impl ThreadArg for TvuReceiveThreadsArg {
    const NAME: &'static str = "tvu_receive_threads";
    const LONG_NAME: &'static str = "tvu-receive-threads";
    const HELP: &'static str =
        "Number of threads (and sockets) to use for receiving shreds on the TVU port";

    fn default() -> usize {
        solana_gossip::cluster_info::DEFAULT_NUM_TVU_SOCKETS.get()
    }
    fn min() -> usize {
        solana_gossip::cluster_info::MINIMUM_NUM_TVU_SOCKETS.get()
    }
}

/// 消息分片验证线程数，默认 1/2 核数
struct TvuShredSigverifyThreadsArg;
impl ThreadArg for TvuShredSigverifyThreadsArg {
    const NAME: &'static str = "tvu_shred_sigverify_threads";
    const LONG_NAME: &'static str = "tvu-shred-sigverify-threads";
    const HELP: &'static str =
        "Number of threads to use for performing signature verification of received shreds";

    fn default() -> usize {
        get_thread_count()
    }
}
