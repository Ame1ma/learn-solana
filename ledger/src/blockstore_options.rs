use {
    crate::blockstore_db::{default_num_compaction_threads, default_num_flush_threads},
    rocksdb::{DBCompressionType as RocksCompressionType, DBRecoveryMode},
    std::num::NonZeroUsize,
};
/// The subdirectory under ledger directory where the Blockstore lives
pub const BLOCKSTORE_DIRECTORY_ROCKS_LEVEL: &str = "rocksdb";
// **RocksDB 的 Compaction（压缩）** 是一个重要的过程，用于管理和优化数据库中的存储结构。它的主要作用是通过合并和整理数据库中的 SST（Sorted String Table）文件来减少磁盘空间的使用，提高查询和写入的效率。压缩过程对于 **键值存储**（如 RocksDB）来说非常关键，因为它有助于确保数据库在长期使用过程中不会变得过于臃肿，并保持良好的性能。
// ### 1. **RocksDB 中的数据存储结构**
// RocksDB 是一个高性能的嵌入式键值数据库，它基于 **LSM（Log-Structured Merge）树** 数据结构来组织数据。LSM 树的主要特点是将写入操作追加到一个日志中，然后通过定期压缩和合并数据来优化存储。
// RocksDB 中的数据存储分为几个层级：
// - **MemTable**：这是内存中的数据结构，存储着最近的写入。数据会被写入 MemTable，当 MemTable 满时，它会被转储到磁盘上的 SST 文件中。
// - **SST Files**：当 MemTable 被写满时，它会被转储为一个 SST 文件，存储在磁盘上。每个 SST 文件都包含了一段有序的键值对。
// - **Levels**：RocksDB 将 SST 文件分为多个层级，每个层级包含一定数量的 SST 文件。这些文件会随着时间推移在不同层级间进行压缩和合并。
// ### 2. **Compaction 的作用**
// 压缩是为了优化 RocksDB 中的磁盘存储和性能。它通过将多个 SST 文件合并成更少的文件来减少磁盘的占用，并提高查询效率。压缩的主要目标有两个：
// 1. **减少碎片化**：随着数据的写入和删除，SST 文件会逐渐积累，并且一些旧的文件可能会包含无效的、已删除的键值对。压缩可以清理这些过时的数据，减少磁盘空间的浪费。
// 2. **提高查询性能**：每当查询请求到达时，RocksDB 需要查找相关的 SST 文件。如果 SST 文件太多，查询会变得缓慢。通过压缩，将多个小文件合并成一个较大的文件，查询性能得到提升。
// ### 3. **Compaction 的过程**
// RocksDB 的压缩过程通常会在后台自动进行，但也可以根据需要进行配置。压缩过程包括以下几个步骤：
// - **选择需要压缩的文件**：根据一定的策略，RocksDB 会选择某些层级中的文件进行压缩。通常，RocksDB 会在较低层级（如 Level 0）中发现许多小的 SST 文件，这些文件可能会包含重复的或过期的键值对。
// - **合并文件**：RocksDB 会将选中的多个 SST 文件合并，并根据需要去除其中过期或删除的键值对。合并时，RocksDB 会根据键的顺序重新排列数据。
// - **写入新的文件**：合并后的数据会写入新的 SST 文件中，而旧的文件会被删除。
// - **删除过时数据**：在压缩过程中，RocksDB 会删除所有已过期的键值对，或者那些已经被标记为删除的数据。这样，数据库的存储空间就会被整理和优化。
// ### 4. **Compaction 的策略**
// RocksDB 提供了多种压缩策略，可以根据实际需求进行配置。常见的压缩策略包括：
// - **Level-based Compaction（基于层级的压缩）**：在 Level 0 层级中，RocksDB 会将多个 SST 文件合并成一个更大的文件，并将其移到 Level 1。随着层级逐渐增大，压缩的粒度也会越来越大。
// - **Universal Compaction（通用压缩）**：这种策略通常用于频繁更新的场景。它不按照层级进行压缩，而是通过一定的阈值来决定哪些文件需要压缩。它更适合写负载较重的场景，但可能导致更多的磁盘 I/O 操作。
// - **FIFO Compaction（先进先出压缩）**：根据文件的创建时间进行压缩。较旧的文件会被优先压缩，适用于存储场景中不关心数据的长期保留。
// - **Size-tiered Compaction（基于大小的压缩）**：当某个层级中的 SST 文件达到一定大小时，RocksDB 会将这些文件合并成更大的文件，直到达到层级限制。
// ### 5. **Compaction 的触发条件**
// 压缩通常会在以下几种情况下触发：
// - **MemTable 满了**：当 MemTable 中的数据积累到一定程度时，它会被转储到磁盘，生成一个新的 SST 文件。这个操作会触发基于层级的压缩策略。
// - **文件数量或大小达到阈值**：当某个层级中的 SST 文件数量或文件大小超过设定的阈值时，RocksDB 会启动压缩操作来整理这些文件。
// - **后台自动压缩**：RocksDB 会在后台自动进行压缩工作，以确保数据的存储和查询性能。
// ### 6. **Compaction 的性能影响**
// 虽然压缩操作有助于减少磁盘空间的使用并提高查询性能，但它本身也是一个资源密集型操作。压缩过程中可能会导致以下性能影响：
// - **磁盘 I/O**：压缩会涉及大量的磁盘读取和写入操作，这可能会占用大量的 I/O 带宽。
// - **CPU 使用**：合并和排序操作会消耗一定的 CPU 资源。
// - **延迟波动**：由于后台进行压缩时需要占用系统资源，可能会出现一定的延迟波动，尤其是在压缩操作较为频繁时。
// 因此，合理配置压缩策略和压缩的触发条件非常重要，以平衡存储优化和性能影响。
// ### 7. **总结**
// RocksDB 的压缩（Compaction）是其性能优化的核心机制之一，它通过定期合并和整理 SST 文件来减少磁盘空间的浪费，提高查询性能。压缩的过程包括文件合并、删除无效数据和优化存储结构。虽然压缩有助于提升性能，但它也可能带来一定的资源消耗，特别是在高负载情况下。因此，在使用 RocksDB 时，了解压缩过程的工作原理并适当调整配置是非常重要的。

// **SST（Sorted String Table）** 文件是 **RocksDB** 等键值数据库（Key-Value Store）中用来存储和组织数据的基本文件格式。它是基于 **LSM（Log-Structured Merge）树** 数据结构的核心部分之一，通常用于持久化存储已经被写入到数据库的数据。
// ### 1. **SST 文件概述**
// SST 文件包含了已排序的键值对，通常是数据库中的一个数据快照。SST 文件格式的设计使得数据存储高效并适合进行快速查询操作。每个 SST 文件存储一段有序的键值对，并且这些文件在磁盘中按层次组织，以提高查询效率和合并压缩过程的性能。
// ### 2. **SST 文件的结构**
// 一个典型的 SST 文件通常包含以下几个部分：
// - **元数据（Meta Data）**：包含一些关于文件的信息，比如文件的最大和最小键值，文件的大小等。这些元数据用于优化读取操作和压缩过程。
// - **数据块（Data Blocks）**：包含实际的键值对数据。键值对是有序存储的，每个数据块通常会被压缩，以减小磁盘占用空间。
// - **索引块（Index Block）**：为数据块中的键提供索引，以加速查找操作。它帮助快速定位特定键值对所在的数据块。
// - **Bloom Filter**：一种用于判断某个键是否存在的概率数据结构。它可以帮助在查询过程中避免不必要的磁盘 I/O，减少读取的次数。
// - **过滤器（Filters）**：类似于 Bloom Filter，但可以更灵活地设计，用于进一步优化查找过程。
// ### 3. **SST 文件的生成和存储**
// - **MemTable 转储为 SST 文件**：当 **MemTable**（内存中的数据结构）达到一定的容量时，它会被转储为 SST 文件，写入到磁盘上。这个操作会生成一个新的 SST 文件，包含当前内存中的有序键值对。
//   - **MemTable** 是一个内存中的数据结构，当有新的写入请求到来时，数据会先写入 MemTable 中。
//   - 一旦 MemTable 被填满，它就会被持久化到磁盘，形成一个新的 SST 文件。
// - **Level 的层次结构**：RocksDB 会将这些 SST 文件根据时间和大小分层存储。每个层级的文件会有不同的大小限制，RocksDB 会将低层次的 SST 文件合并到更高层次的文件中，以保证性能。每个层级的 SST 文件都会包含有序的键值对，这样能够高效地执行范围查询和查找。
// ### 4. **SST 文件的压缩和合并**
// RocksDB 使用 **LSM 树**（Log-Structured Merge Tree）作为存储结构。LSM 树通过将新的写入操作追加到 MemTable，再将 MemTable 转储为 SST 文件来减少磁盘碎片。在这些文件转储到磁盘后，RocksDB 会周期性地进行压缩（Compaction），合并和整理多个 SST 文件，以便：
// - 删除已删除或过期的键。
// - 整理文件，避免文件碎片过多，保持存储效率。
// - 将较小的 SST 文件合并为较大的文件，减少文件数量，提高查询性能。
// ### 5. **SST 文件的优点**
// - **高效的读写操作**：SST 文件的设计使得写操作非常高效，且通过合适的索引和过滤器，查询操作也能够快速定位目标数据。
// - **持久化和恢复**：由于 SST 文件是以有序的方式存储数据，它们可以直接作为持久化存储的格式，并且支持数据库的高效恢复。
// - **低延迟查询**：通过索引和 Bloom Filter，SST 文件能够加速查询，减少不必要的磁盘 I/O。
// ### 6. **SST 文件的缺点**
// - **空间开销**：由于数据在 SST 文件中是按顺序存储的，因此一旦数据发生删除或更新，会在合并时产生冗余数据。虽然后期会通过压缩过程清理这些数据，但短期内可能会占用较多的空间。
// - **写放大**：虽然写入数据时非常高效，但由于 LSM 树的结构和压缩机制，可能会导致多个写入操作被合并并写入多个 SST 文件，这会引起所谓的 **写放大效应**（Write Amplification），增加 I/O 的次数。
// ### 7. **SST 文件与 RocksDB 的关系**
// 在 RocksDB 中，SST 文件是数据存储的核心组成部分。当 RocksDB 启动时，它会加载多个 SST 文件来恢复数据，并通过 MemTable 和写日志来处理新的写入请求。RocksDB 会通过定期的压缩操作将多个 SST 文件合并成更大的文件，提升存储效率并优化查询性能。
// ### 8. **SST 文件的查询**
// - **查找键值对**：RocksDB 在查询时，会根据键值的范围，首先定位到可能的 SST 文件，然后通过索引块和过滤器来减少磁盘 I/O，快速找到目标键值对。
// - **范围查询**：RocksDB 的设计使得范围查询变得非常高效，因为键值对是有序存储的，可以通过合并不同层级的 SST 文件来实现高效的范围查询。
// ### 9. **总结**
// SST 文件（Sorted String Table 文件）是 RocksDB 存储数据的核心格式，包含有序的键值对。通过将数据以这种格式存储，RocksDB 能够提供高效的写入、查询和压缩操作。SST 文件的设计使得 RocksDB 在处理大量数据时，能够保持良好的性能和扩展性，同时通过压缩机制优化存储。

/// 块存储选项
#[derive(Debug, Clone)]
pub struct BlockstoreOptions {
    // The access type of blockstore. Default: Primary
    /// 访问类型，独占访问还是共享访问
    pub access_type: AccessType,
    // Whether to open a blockstore under a recovery mode. Default: None.
    /// 是否启用恢复模式，以及用哪种恢复模式
    pub recovery_mode: Option<BlockstoreRecoveryMode>,
    // When opening the Blockstore, determines whether to error or not if the
    // desired open file descriptor limit cannot be configured. Default: true.
    /// 主要用于在打开 Blockstore 时，决定是否强制检查和限制文件描述符（file descriptor）数量的上限。
    pub enforce_ulimit_nofile: bool,
    /// 账本列选项，列压缩类型和性能监控采样频率
    pub column_options: LedgerColumnOptions,
    /// 用来压缩的线程数
    pub num_rocksdb_compaction_threads: NonZeroUsize,
    /// 用来刷新的线程数
    pub num_rocksdb_flush_threads: NonZeroUsize,
}
impl Default for BlockstoreOptions {
    /// The default options are the values used by [`Blockstore::open`].
    ///
    /// [`Blockstore::open`]: crate::blockstore::Blockstore::open
    fn default() -> Self {
        Self {
            access_type: AccessType::Primary,
            recovery_mode: None,
            enforce_ulimit_nofile: true,
            column_options: LedgerColumnOptions::default(),
            num_rocksdb_compaction_threads: default_num_compaction_threads(),
            num_rocksdb_flush_threads: default_num_flush_threads(),
        }
    }
}
impl BlockstoreOptions {
    pub fn default_for_tests() -> Self {
        Self {
            // No need to enforce the limit in tests
            enforce_ulimit_nofile: false,
            ..BlockstoreOptions::default()
        }
    }
}
/// 块存储访问类型
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessType {
    /// Primary (read/write) access; only one process can have Primary access.
    /// 独占访问，写访问
    Primary,
    /// Primary (read/write) access with RocksDB automatic compaction disabled.
    /// 独占访问，写访问，禁用自动压缩
    PrimaryForMaintenance,
    /// Secondary (read) access; multiple processes can have Secondary access.
    /// Additionally, Secondary access can be obtained while another process
    /// already has Primary access.
    /// 共享访问，读访问
    Secondary,
}
/// 当 Solana 节点在处理或恢复区块存储时遇到问题或损坏的记录时，应该采取的恢复策略。
#[derive(Debug, Clone)]
pub enum BlockstoreRecoveryMode {
    /// 在区块存储恢复过程中，如果尾部记录（通常是最近的或最末尾的记录）损坏，节点会继续恢复并使用其他可用的记录。
    /// 它会容忍损坏的尾部记录，不会立即终止恢复过程。
    /// 这种模式适用于那些允许少量损坏的场景，重点是尽量保持节点运行并恢复可用的状态。
    TolerateCorruptedTailRecords,
    /// 恢复过程中要求严格的完整性和一致性。即使只遇到一个损坏的记录，恢复过程也会失败，不会继续。
    /// 这种模式适用于那些需要 100% 数据一致性 的场景，要求每一块数据都要完整且一致。
    /// 如果发现任何损坏的记录，节点将会中止恢复过程，确保不会继续使用任何不一致的数据。
    AbsoluteConsistency,
    /// 这种恢复模式会将区块存储恢复到某个特定的时间点。
    /// 这意味着如果在该时间点之后的记录损坏，系统将会回滚到一个有效的时间点，忽略后续的损坏记录。
    /// 这种方式有助于将恢复过程限制在某个历史时刻，减少由损坏的记录引发的数据丢失。
    PointInTime,
    /// 如果恢复过程中遇到任何损坏的记录，该记录将会被跳过，并继续处理后面的记录。
    /// 节点不会停止恢复，而是尽量跳过损坏的数据，继续使用其他完整的区块记录。
    /// 这个模式适用于那些容忍丢失部分数据的场景，可以在数据出现问题时保持较高的恢复速度。
    SkipAnyCorruptedRecord,
}
impl From<&str> for BlockstoreRecoveryMode {
    fn from(string: &str) -> Self {
        match string {
            "tolerate_corrupted_tail_records" => {
                BlockstoreRecoveryMode::TolerateCorruptedTailRecords
            }
            "absolute_consistency" => BlockstoreRecoveryMode::AbsoluteConsistency,
            "point_in_time" => BlockstoreRecoveryMode::PointInTime,
            "skip_any_corrupted_record" => BlockstoreRecoveryMode::SkipAnyCorruptedRecord,
            bad_mode => panic!("Invalid recovery mode: {bad_mode}"),
        }
    }
}
impl From<BlockstoreRecoveryMode> for DBRecoveryMode {
    fn from(brm: BlockstoreRecoveryMode) -> Self {
        match brm {
            BlockstoreRecoveryMode::TolerateCorruptedTailRecords => {
                DBRecoveryMode::TolerateCorruptedTailRecords
            }
            BlockstoreRecoveryMode::AbsoluteConsistency => DBRecoveryMode::AbsoluteConsistency,
            BlockstoreRecoveryMode::PointInTime => DBRecoveryMode::PointInTime,
            BlockstoreRecoveryMode::SkipAnyCorruptedRecord => {
                DBRecoveryMode::SkipAnyCorruptedRecord
            }
        }
    }
}
/// Options for LedgerColumn.
/// Each field might also be used as a tag that supports group-by operation when
/// reporting metrics.
/// 账本列选项
#[derive(Default, Debug, Clone)]
pub struct LedgerColumnOptions {
    // Determine the way to compress column families which are eligible for
    // compression.
    /// 列的压缩算法
    pub compression_type: BlockstoreCompressionType,
    // Control how often RocksDB read/write performance samples are collected.
    // If the value is greater than 0, then RocksDB read/write perf sample
    // will be collected once for every `rocks_perf_sample_interval` ops.
    /// rockdbs 性能监控采样频率
    pub rocks_perf_sample_interval: usize,
}
impl LedgerColumnOptions {
    pub fn get_compression_type_string(&self) -> &'static str {
        match self.compression_type {
            BlockstoreCompressionType::None => "None",
            BlockstoreCompressionType::Snappy => "Snappy",
            BlockstoreCompressionType::Lz4 => "Lz4",
            BlockstoreCompressionType::Zlib => "Zlib",
        }
    }
}
#[derive(Debug, Clone)]
pub enum BlockstoreCompressionType {
    None,
    Snappy,
    Lz4,
    Zlib,
}
impl Default for BlockstoreCompressionType {
    fn default() -> Self {
        Self::None
    }
}
impl BlockstoreCompressionType {
    pub(crate) fn to_rocksdb_compression_type(&self) -> RocksCompressionType {
        match self {
            Self::None => RocksCompressionType::None,
            Self::Snappy => RocksCompressionType::Snappy,
            Self::Lz4 => RocksCompressionType::Lz4,
            Self::Zlib => RocksCompressionType::Zlib,
        }
    }
}
