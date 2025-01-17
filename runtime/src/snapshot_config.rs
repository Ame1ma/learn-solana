use {
    crate::{
        snapshot_bank_utils,
        snapshot_utils::{self, ArchiveFormat, SnapshotVersion},
    },
    solana_sdk::clock::Slot,
    std::{num::NonZeroUsize, path::PathBuf},
};

/// Snapshot configuration and runtime information
/// 快照配置
#[derive(Clone, Debug)]
pub struct SnapshotConfig {
    /// Specifies the ways thats snapshots are allowed to be used
    /// 快照用途，只在启动时加载快照，还是同时在运行时保存新快照
    pub usage: SnapshotUsage,

    /// Generate a new full snapshot archive every this many slots
    /// 多少时隙归档一次全快照
    pub full_snapshot_archive_interval_slots: Slot,

    /// Generate a new incremental snapshot archive every this many slots
    /// 多少时隙归档一次增量快照
    pub incremental_snapshot_archive_interval_slots: Slot,

    /// Path to the directory where full snapshot archives are stored
    /// 全快照路径
    pub full_snapshot_archives_dir: PathBuf,

    /// Path to the directory where incremental snapshot archives are stored
    /// 增量快照路径
    pub incremental_snapshot_archives_dir: PathBuf,

    /// Path to the directory where bank snapshots are stored
    /// 银行快照路径
    pub bank_snapshots_dir: PathBuf,

    /// The archive format to use for snapshots
    /// 快照压缩格式
    pub archive_format: ArchiveFormat,

    /// Snapshot version to generate
    /// 快照版本
    pub snapshot_version: SnapshotVersion,

    /// Maximum number of full snapshot archives to retain
    /// 全快照保存大小限制
    pub maximum_full_snapshot_archives_to_retain: NonZeroUsize,

    /// Maximum number of incremental snapshot archives to retain
    /// NOTE: Incremental snapshots will only be kept for the latest full snapshot
    /// 增量快照保存大小限制
    pub maximum_incremental_snapshot_archives_to_retain: NonZeroUsize,

    /// This is the `debug_verify` parameter to use when calling `update_accounts_hash()`
    /// 账户哈希 debug 验证
    /// 强制 Solana 在运行时对账户哈希进行 验证，以确保账户数据库中保存的每个账户的哈希值是 正确的，
    /// 并且与实际存储的账户状态一致。启用此选项后，Solana 会进行额外的哈希校验，确保账户的哈希值没有损坏或不一致。
    pub accounts_hash_debug_verify: bool,

    // Thread niceness adjustment for snapshot packager service
    /// 快照打包器优先级
    pub packager_thread_niceness_adj: i8,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            usage: SnapshotUsage::LoadAndGenerate,
            full_snapshot_archive_interval_slots:
                snapshot_bank_utils::DEFAULT_FULL_SNAPSHOT_ARCHIVE_INTERVAL_SLOTS,
            incremental_snapshot_archive_interval_slots:
                snapshot_bank_utils::DEFAULT_INCREMENTAL_SNAPSHOT_ARCHIVE_INTERVAL_SLOTS,
            full_snapshot_archives_dir: PathBuf::default(),
            incremental_snapshot_archives_dir: PathBuf::default(),
            bank_snapshots_dir: PathBuf::default(),
            archive_format: ArchiveFormat::TarZstd,
            snapshot_version: SnapshotVersion::default(),
            maximum_full_snapshot_archives_to_retain:
                snapshot_utils::DEFAULT_MAX_FULL_SNAPSHOT_ARCHIVES_TO_RETAIN,
            maximum_incremental_snapshot_archives_to_retain:
                snapshot_utils::DEFAULT_MAX_INCREMENTAL_SNAPSHOT_ARCHIVES_TO_RETAIN,
            accounts_hash_debug_verify: false,
            packager_thread_niceness_adj: 0,
        }
    }
}

impl SnapshotConfig {
    /// A new snapshot config used for only loading at startup
    #[must_use]
    pub fn new_load_only() -> Self {
        Self {
            usage: SnapshotUsage::LoadOnly,
            ..Self::default()
        }
    }

    /// Should snapshots be generated?
    #[must_use]
    pub fn should_generate_snapshots(&self) -> bool {
        self.usage == SnapshotUsage::LoadAndGenerate
    }
}

/// Specify the ways that snapshots are allowed to be used
/// 快照用途
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SnapshotUsage {
    /// Snapshots are only used at startup, to load the accounts and bank
    /// 快照仅在启动时使用，用于加载帐户和银行
    LoadOnly,
    /// Snapshots are used everywhere; both at startup (i.e. load) and steady-state (i.e.
    /// generate).  This enables taking snapshots.
    /// 快照无处不在；在启动（即负载）和稳态（生成)。这允许拍摄快照。
    LoadAndGenerate,
}
