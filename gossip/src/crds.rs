//! This module implements Cluster Replicated Data Store for
//! asynchronous updates in a distributed network.
//!
//! Data is stored in the CrdsValue type, each type has a specific
//! CrdsValueLabel.  Labels are semantically grouped into a single record
//! that is identified by a Pubkey.
//! * 1 Pubkey maps many CrdsValueLabels
//! * 1 CrdsValueLabel maps to 1 CrdsValue
//!
//! The Label, the record Pubkey, and all the record labels can be derived
//! from a single CrdsValue.
//!
//! The actual data is stored in a single map of
//! `CrdsValueLabel(Pubkey) -> CrdsValue` This allows for partial record
//! updates to be propagated through the network.
//!
//! This means that full `Record` updates are not atomic.
//!
//! Additional labels can be added by appending them to the CrdsValueLabel,
//! CrdsValue enums.
//!
//! Merge strategy is implemented in:
//!     fn overrides(value: &CrdsValue, other: &VersionedCrdsValue) -> bool
//!
//! A value is updated to a new version if the labels match, and the value
//! wallclock is later, or the value hash is greater.

//! `CRDS`（Cluster Replicated Data Store）是 Solana 网络中的一个重要模块，负责在分布式网络中以异步方式更新和存储数据。它采用了一种分布式数据库的设计，用于处理 Solana 节点之间的状态同步和信息传播。下面将详细介绍 CRDS 的工作原理及其关键概念。
//! ### 1. **CRDS 结构**
//! 在 Solana 中，CRDS 负责管理和存储有关网络的元数据。每个节点（Validator）都会维护一份 CRDS 表，其中包含了它所了解的各种信息。这些信息是由 `CrdsValue` 类型的值组成，而这些值有特定的标签（`CrdsValueLabel`）来标识。
//! ### 2. **数据存储方式**
//! 在 CRDS 中，数据是通过以下关系来组织和存储的：
//! - **1 Pubkey 对应多个 CrdsValueLabels**
//! - **1 CrdsValueLabel 对应一个 CrdsValue**
//! 一个节点的 `Pubkey` 是唯一的，它作为一组记录的标识符。这些记录包含了多个标签（`CrdsValueLabel`），每个标签映射到一个具体的值（`CrdsValue`）。这些标签被归类到同一个记录中。
//! 例如：
//! - 一个节点的 `Pubkey` 可能会有多个 `CrdsValueLabel`，每个标签表示网络中的某些信息（如节点的投票、状态等）。
//! - 每个 `CrdsValueLabel` 映射到一个具体的值（`CrdsValue`），这些值可以是有关该节点的具体信息。
//! ### 3. **数据存储与更新**
//! 数据是通过 `CrdsValueLabel(Pubkey) -> CrdsValue` 这种映射方式存储的。这种结构允许数据的部分更新传播，而不需要整个记录的更新。例如，如果某个节点更新了它的一部分信息（如投票信息），它只会更新对应的 `CrdsValueLabel`，而不需要修改整个节点的记录。
//! #### 关键点：
//! - **部分记录更新**：Solana 支持部分记录更新，这意味着每个标签（`CrdsValueLabel`）的更新是独立的，不会影响整个记录的其他部分。
//! - **记录更新不是原子性的**：由于记录中的数据是分散存储的，更新操作是局部的，因此不是一个原子操作。部分数据更新会被传播到网络中，其他节点可以逐渐获得更新。
//! ### 4. **标签和枚举**
//! CRDS 中的标签和枚举是非常灵活的，允许动态地为记录添加新的标签。例如，如果一个节点需要存储额外的元数据，只需要将新的标签添加到 `CrdsValueLabel` 和 `CrdsValue` 枚举中。这使得 CRDS 能够在不同的场景中适应各种需求。
//! ### 5. **数据合并策略**
//! 为了确保数据一致性和合并不同版本的数据，CRDS 中实现了一个合并策略。合并策略的实现函数是：`fn overrides(value: &CrdsValue, other: &VersionedCrdsValue) -> bool`。
//! 合并策略的基本原则如下：
//! - **更新条件**：如果新版本的数据标签匹配，并且它的 **wallclock 时间**（表示数据的更新时间）比现有数据的 wallclock 时间晚，或者它的 **数据哈希**（用于表示数据的版本）更大，那么新数据就会覆盖旧数据。
//!   这意味着，数据的版本比较不仅仅基于时间，还包括数据本身的哈希值。这个机制保证了网络中数据的一致性，并且避免了不必要的冲突。
//! ### 6. **工作流程**
//! 在 Solana 中，CRDS 通过 Gossip 协议在节点之间传播这些数据。每个节点维护一份 CRDS 数据库，其中包括了网络中其他节点的状态信息、投票情况、区块信息等。节点通过以下方式进行数据同步：
//! 1. **节点通过 Gossip 接收数据**：节点接收到其他节点广播的更新信息时，会根据 CRDS 的合并策略对自己的 CRDS 数据库进行更新。
//! 2. **局部更新传播**：每次更新都只是针对某个特定标签（`CrdsValueLabel`）的数据进行更新，而不是整个记录。这减少了网络负载，并且提高了数据传播的效率。
//! ### 7. **应用场景**
//! CRDS 在 Solana 中的应用场景主要是：
//! - **节点信息同步**：验证者节点会通过 CRDS Gossip 协议与其他节点同步其投票信息、状态信息等。
//! - **网络状态管理**：每个节点通过 CRDS 表了解网络中其他节点的状态，以便做出投票、选择区块等决策。
//! - **分布式状态更新**：由于 CRDS 支持部分数据更新和局部合并，它适合于 Solana 这样的高频交易环境，能够迅速传播最新的网络状态。
//! ### 8. **CRDS 的优点**
//! - **高效的网络通信**：CRDS 使用 Gossip 协议高效传播信息，通过局部更新减少了带宽消耗。
//! - **去中心化**：每个节点都有自己的 CRDS 数据库，没有中央数据库或控制点，这增强了网络的容错能力。
//! - **动态扩展性**：CRDS 允许在不改变整个数据结构的情况下，向记录中添加新的标签，从而适应网络中的不同需求。
//! - **容错和一致性**：由于采用合并策略，CRDS 保证了数据在不同节点之间的一致性，避免了版本冲突。
//! ### 总结
//! CRDS 是 Solana 网络中的一个分布式数据库模块，负责以高效和异步的方式在验证者节点之间传播网络状态和元数据。它通过 `CrdsValue` 和 `CrdsValueLabel` 实现了灵活的数据存储和更新机制，使得节点能够高效地同步和更新彼此的状态。CRDS 的设计使得 Solana 网络具备高容错性和高扩展性，支持高吞吐量和低延迟的分布式系统。

use {
    crate::{
        contact_info::ContactInfo,
        crds_data::CrdsData,
        crds_entry::CrdsEntry,
        crds_gossip_pull::CrdsTimeouts,
        crds_shards::CrdsShards,
        crds_value::{CrdsValue, CrdsValueLabel},
    },
    assert_matches::debug_assert_matches,
    indexmap::{
        map::{rayon::ParValues, Entry, IndexMap},
        set::IndexSet,
    },
    lru::LruCache,
    rayon::{prelude::*, ThreadPool},
    solana_sdk::{clock::Slot, hash::Hash, pubkey::Pubkey, signature::Signature},
    std::{
        cmp::Ordering,
        collections::{hash_map, BTreeMap, HashMap, VecDeque},
        ops::{Bound, Index, IndexMut},
        sync::Mutex,
    },
};

const CRDS_SHARDS_BITS: u32 = 12;
// Number of vote slots to track in an lru-cache for metrics.
const VOTE_SLOTS_METRICS_CAP: usize = 100;
// Required number of leading zero bits for crds signature to get reported to influx
// mean new push messages received per minute per node
//      testnet: ~500k,
//      mainnet: ~280k
// target: 1 signature reported per minute
// log2(500k) = ~18.9.
const SIGNATURE_SAMPLE_LEADING_ZEROS: u32 = 19;

/// CRDS（Cluster Radio Gossip Database System）
/// 在 Solana 中，CRDS Gossip 的作用是将网络中的信息（如验证者信息、交易状态等）通过 Gossip 协议广播到其他节点。
/// 具体来说，CRDS Gossip 主要负责以下几个方面的信息传播：
/// 节点状态信息：包括节点是否在线、节点的性能、节点是否处于领导状态等。
/// 区块和投票信息：验证者投票的结果、区块是否被确认、验证者的投票情况等。
/// 网络健康状态：比如当前网络是否有分叉，或者节点是否存在延迟等。
pub struct Crds {
    /// Stores the map of labels and values
    /// 按插入序排序的键值表
    /// 对crds的值是分种类存放在这个键值表里的
    /// crds种类映射到值
    table: IndexMap<CrdsValueLabel, VersionedCrdsValue>,
    /// 当前插入位置的游标
    cursor: Cursor, // Next insert ordinal location.
    /// crds 消息分片
    shards: CrdsShards,
    nodes: IndexSet<usize>, // Indices of nodes' ContactInfo.
    // Indices of Votes keyed by insert order.
    votes: BTreeMap<u64 /*insert order*/, usize /*index*/>,
    // Indices of EpochSlots keyed by insert order.
    epoch_slots: BTreeMap<u64 /*insert order*/, usize /*index*/>,
    // Indices of DuplicateShred keyed by insert order.
    duplicate_shreds: BTreeMap<u64 /*insert order*/, usize /*index*/>,
    // Indices of all crds values associated with a node.
    records: HashMap<Pubkey, IndexSet<usize>>,
    // Indices of all entries keyed by insert order.
    entries: BTreeMap<u64 /*insert order*/, usize /*index*/>,
    // Hash of recently purged values.
    purged: VecDeque<(Hash, u64 /*timestamp*/)>,
    // Mapping from nodes' pubkeys to their respective shred-version.
    shred_versions: HashMap<Pubkey, u16>,
    stats: Mutex<CrdsStats>,
}

#[derive(PartialEq, Eq, Debug)]
pub enum CrdsError {
    DuplicatePush(/*num dups:*/ u8),
    InsertFailed,
    UnknownStakes,
}

#[derive(Clone, Copy)]
pub enum GossipRoute<'a> {
    LocalMessage,
    PullRequest,
    PullResponse,
    PushMessage(/*from:*/ &'a Pubkey),
}

type CrdsCountsArray = [usize; 14];

/// crds数据统计
pub(crate) struct CrdsDataStats {
    pub(crate) counts: CrdsCountsArray,
    pub(crate) fails: CrdsCountsArray,
    pub(crate) votes: LruCache<Slot, /*count:*/ usize>,
}

/// crds 统计信息
#[derive(Default)]
pub(crate) struct CrdsStats {
    /// 拉取统计
    pub(crate) pull: CrdsDataStats,
    /// 发送统计
    pub(crate) push: CrdsDataStats,
    /// number of times a message was first received via a PullResponse
    /// and that message was later received via a PushMessage
    /// 冗余拉取响应次数
    pub(crate) num_redundant_pull_responses: u64,
    /// 重复发送消息次数
    pub(crate) num_duplicate_push_messages: u64,
}

/// This structure stores some local metadata associated with the CrdsValue
/// 版本化 crds 值
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct VersionedCrdsValue {
    /// Ordinal index indicating insert order.
    /// 插入顺序
    ordinal: u64,
    /// crds 值
    pub value: CrdsValue,
    /// local time when updated
    /// 升级时的本地时间戳
    pub(crate) local_timestamp: u64,
    /// None -> value upserted by GossipRoute::{LocalMessage,PullRequest}
    /// Some(0) -> value upserted by GossipRoute::PullResponse
    /// Some(k) if k > 0 -> value upserted by GossipRoute::PushMessage w/ k - 1 push duplicates
    /// 用于追踪 CRDS 数据项在 Gossip 协议中通过推送消息接收的次数
    num_push_recv: Option<u8>,
}

/// 游标，指向crds表中将要发送的项
#[derive(Clone, Copy, Default)]
pub struct Cursor(u64);

impl Cursor {
    /// 当前位置
    fn ordinal(&self) -> u64 {
        self.0
    }

    // Updates the cursor position given the ordinal index of value consumed.
    /// 推进游标
    #[inline]
    fn consume(&mut self, ordinal: u64) {
        self.0 = self.0.max(ordinal + 1);
    }
}

impl VersionedCrdsValue {
    /// 新建 crds 值
    fn new(value: CrdsValue, cursor: Cursor, local_timestamp: u64, route: GossipRoute) -> Self {
        /// 根据方向不同，统计通过推送消息接收的次数
        let num_push_recv = match route {
            GossipRoute::LocalMessage => None,
            GossipRoute::PullRequest => None,
            GossipRoute::PullResponse => Some(0),
            GossipRoute::PushMessage(_) => Some(1),
        };

        VersionedCrdsValue {
            ordinal: cursor.ordinal(),
            value,
            local_timestamp,
            num_push_recv,
        }
    }
}

impl Default for Crds {
    fn default() -> Self {
        Crds {
            table: IndexMap::default(),
            cursor: Cursor::default(),
            shards: CrdsShards::new(CRDS_SHARDS_BITS),
            nodes: IndexSet::default(),
            votes: BTreeMap::default(),
            epoch_slots: BTreeMap::default(),
            duplicate_shreds: BTreeMap::default(),
            records: HashMap::default(),
            entries: BTreeMap::default(),
            purged: VecDeque::default(),
            shred_versions: HashMap::default(),
            stats: Mutex::<CrdsStats>::default(),
        }
    }
}

// Returns true if the first value updates the 2nd one.
// Both values should have the same key/label.
fn overrides(value: &CrdsValue, other: &VersionedCrdsValue) -> bool {
    assert_eq!(value.label(), other.value.label(), "labels mismatch!");
    // Contact-infos and node instances are special cased so that if there are
    // two running instances of the same node, the more recent start is
    // propagated through gossip regardless of wallclocks.
    match value.data() {
        CrdsData::ContactInfo(value) => {
            if let CrdsData::ContactInfo(other) = other.value.data() {
                if let Some(out) = value.overrides(other) {
                    return out;
                }
            }
        }
        CrdsData::NodeInstance(value) => {
            if let CrdsData::NodeInstance(other) = other.value.data() {
                if let Some(out) = value.overrides(other) {
                    return out;
                }
            }
        }
        _ => (),
    }
    match value.wallclock().cmp(&other.value.wallclock()) {
        Ordering::Less => false,
        Ordering::Greater => true,
        // Ties should be broken in a deterministic way across the cluster.
        // For backward compatibility this is done by comparing hash of
        // serialized values.
        Ordering::Equal => other.value.hash() < value.hash(),
    }
}

impl Crds {
    /// Returns true if the given value updates an existing one in the table.
    /// The value is outdated and fails to insert, if it already exists in the
    /// table with a more recent wallclock.
    pub(crate) fn upserts(&self, value: &CrdsValue) -> bool {
        match self.table.get(&value.label()) {
            Some(other) => overrides(value, other),
            None => true,
        }
    }

    /// 插入 crds 表
    pub fn insert(
        &mut self,
        value: CrdsValue,
        now: u64,
        route: GossipRoute,
    ) -> Result<(), CrdsError> {
        /// 标签从值里面取
        let label = value.label();
        /// 公钥
        let pubkey = value.pubkey();
        /// 新建值
        let value = VersionedCrdsValue::new(value, self.cursor, now, route);
        /// 统计信息
        let mut stats = self.stats.lock().unwrap();
        /// 表里找到这个种类
        match self.table.entry(label) {
            /// 空槽，映射中没有对应的键
            Entry::Vacant(entry) => {
                /// 计入统计
                stats.record_insert(&value, route);
                /// 可插入键值对的索引位置
                let entry_index = entry.index();
                self.shards.insert(entry_index, &value);
                match value.value.data() {
                    CrdsData::ContactInfo(node) => {
                        self.nodes.insert(entry_index);
                        self.shred_versions.insert(pubkey, node.shred_version());
                    }
                    CrdsData::Vote(_, _) => {
                        self.votes.insert(value.ordinal, entry_index);
                    }
                    CrdsData::EpochSlots(_, _) => {
                        self.epoch_slots.insert(value.ordinal, entry_index);
                    }
                    CrdsData::DuplicateShred(_, _) => {
                        self.duplicate_shreds.insert(value.ordinal, entry_index);
                    }
                    _ => (),
                };
                self.entries.insert(value.ordinal, entry_index);
                self.records.entry(pubkey).or_default().insert(entry_index);
                self.cursor.consume(value.ordinal);
                entry.insert(value);
                Ok(())
            }
            Entry::Occupied(mut entry) if overrides(&value.value, entry.get()) => {
                stats.record_insert(&value, route);
                let entry_index = entry.index();
                self.shards.remove(entry_index, entry.get());
                self.shards.insert(entry_index, &value);
                match value.value.data() {
                    CrdsData::ContactInfo(node) => {
                        self.shred_versions.insert(pubkey, node.shred_version());
                        // self.nodes does not need to be updated since the
                        // entry at this index was and stays contact-info.
                        debug_assert_matches!(entry.get().value.data(), CrdsData::ContactInfo(_));
                    }
                    CrdsData::Vote(_, _) => {
                        self.votes.remove(&entry.get().ordinal);
                        self.votes.insert(value.ordinal, entry_index);
                    }
                    CrdsData::EpochSlots(_, _) => {
                        self.epoch_slots.remove(&entry.get().ordinal);
                        self.epoch_slots.insert(value.ordinal, entry_index);
                    }
                    CrdsData::DuplicateShred(_, _) => {
                        self.duplicate_shreds.remove(&entry.get().ordinal);
                        self.duplicate_shreds.insert(value.ordinal, entry_index);
                    }
                    _ => (),
                }
                self.entries.remove(&entry.get().ordinal);
                self.entries.insert(value.ordinal, entry_index);
                // As long as the pubkey does not change, self.records
                // does not need to be updated.
                debug_assert_eq!(entry.get().value.pubkey(), pubkey);
                self.cursor.consume(value.ordinal);
                self.purged.push_back((*entry.get().value.hash(), now));
                entry.insert(value);
                Ok(())
            }
            Entry::Occupied(mut entry) => {
                stats.record_fail(&value, route);
                trace!(
                    "INSERT FAILED data: {:?} new.wallclock: {}",
                    value.value.label(),
                    value.value.wallclock(),
                );
                // Identify if the message is outdated (as opposed to
                // duplicate) by comparing value hashes.
                if entry.get().value.hash() != value.value.hash() {
                    self.purged.push_back((*value.value.hash(), now));
                    Err(CrdsError::InsertFailed)
                } else if matches!(route, GossipRoute::PushMessage(_)) {
                    let entry = entry.get_mut();
                    if entry.num_push_recv == Some(0) {
                        stats.num_redundant_pull_responses += 1;
                    } else {
                        stats.num_duplicate_push_messages += 1;
                    }
                    let num_push_dups = entry.num_push_recv.unwrap_or_default();
                    entry.num_push_recv = Some(num_push_dups.saturating_add(1));
                    Err(CrdsError::DuplicatePush(num_push_dups))
                } else {
                    Err(CrdsError::InsertFailed)
                }
            }
        }
    }

    pub fn get<'a, 'b, V>(&'a self, key: V::Key) -> Option<V>
    where
        V: CrdsEntry<'a, 'b>,
    {
        V::get_entry(&self.table, key)
    }

    pub(crate) fn get_shred_version(&self, pubkey: &Pubkey) -> Option<u16> {
        self.shred_versions.get(pubkey).copied()
    }

    /// Returns all entries which are ContactInfo.
    pub(crate) fn get_nodes(&self) -> impl Iterator<Item = &VersionedCrdsValue> {
        self.nodes.iter().map(move |i| self.table.index(*i))
    }

    /// Returns ContactInfo of all known nodes.
    pub(crate) fn get_nodes_contact_info(&self) -> impl Iterator<Item = &ContactInfo> {
        self.get_nodes().map(|v| match v.value.data() {
            CrdsData::ContactInfo(info) => info,
            _ => panic!("this should not happen!"),
        })
    }

    /// Returns all vote entries inserted since the given cursor.
    /// Updates the cursor as the votes are consumed.
    pub(crate) fn get_votes<'a>(
        &'a self,
        cursor: &'a mut Cursor,
    ) -> impl Iterator<Item = &'a VersionedCrdsValue> {
        let range = (Bound::Included(cursor.ordinal()), Bound::Unbounded);
        self.votes.range(range).map(move |(ordinal, index)| {
            cursor.consume(*ordinal);
            self.table.index(*index)
        })
    }

    /// Returns epoch-slots inserted since the given cursor.
    /// Updates the cursor as the values are consumed.
    pub(crate) fn get_epoch_slots<'a>(
        &'a self,
        cursor: &'a mut Cursor,
    ) -> impl Iterator<Item = &'a VersionedCrdsValue> {
        let range = (Bound::Included(cursor.ordinal()), Bound::Unbounded);
        self.epoch_slots.range(range).map(move |(ordinal, index)| {
            cursor.consume(*ordinal);
            self.table.index(*index)
        })
    }

    /// Returns duplicate-shreds inserted since the given cursor.
    /// Updates the cursor as the values are consumed.
    pub(crate) fn get_duplicate_shreds<'a>(
        &'a self,
        cursor: &'a mut Cursor,
    ) -> impl Iterator<Item = &'a VersionedCrdsValue> {
        let range = (Bound::Included(cursor.ordinal()), Bound::Unbounded);
        self.duplicate_shreds
            .range(range)
            .map(move |(ordinal, index)| {
                cursor.consume(*ordinal);
                self.table.index(*index)
            })
    }

    /// Returns all entries inserted since the given cursor.
    /// 获取游标后的项个数
    pub(crate) fn get_entries<'a>(
        &'a self,
        cursor: &'a mut Cursor,
    ) -> impl Iterator<Item = &'a VersionedCrdsValue> {
        let range = (Bound::Included(cursor.ordinal()), Bound::Unbounded);
        self.entries.range(range).map(move |(&ordinal, &index)| {
            cursor.consume(ordinal);
            self.table.index(index)
        })
    }

    /// Returns all records associated with a pubkey.
    pub(crate) fn get_records(&self, pubkey: &Pubkey) -> impl Iterator<Item = &VersionedCrdsValue> {
        self.records
            .get(pubkey)
            .into_iter()
            .flat_map(|records| records.into_iter())
            .map(move |i| self.table.index(*i))
    }

    /// Returns number of known contact-infos (network size).
    pub(crate) fn num_nodes(&self) -> usize {
        self.nodes.len()
    }

    /// Returns number of unique pubkeys.
    pub(crate) fn num_pubkeys(&self) -> usize {
        self.records.len()
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn values(&self) -> impl Iterator<Item = &VersionedCrdsValue> {
        self.table.values()
    }

    pub(crate) fn par_values(&self) -> ParValues<'_, CrdsValueLabel, VersionedCrdsValue> {
        self.table.par_values()
    }

    pub(crate) fn num_purged(&self) -> usize {
        self.purged.len()
    }

    pub(crate) fn purged(&self) -> impl IndexedParallelIterator<Item = Hash> + '_ {
        self.purged.par_iter().map(|(hash, _)| *hash)
    }

    /// Drops purged value hashes with timestamp less than the given one.
    pub(crate) fn trim_purged(&mut self, timestamp: u64) {
        let count = self
            .purged
            .iter()
            .take_while(|(_, ts)| *ts < timestamp)
            .count();
        self.purged.drain(..count);
    }

    /// Returns all crds values which the first 'mask_bits'
    /// of their hash value is equal to 'mask'.
    /// Excludes deprecated values.
    pub(crate) fn filter_bitmask(
        &self,
        mask: u64,
        mask_bits: u32,
    ) -> impl Iterator<Item = &VersionedCrdsValue> {
        self.shards
            .find(mask, mask_bits)
            .map(move |i| self.table.index(i))
            .filter(|VersionedCrdsValue { value, .. }| !value.data().is_deprecated())
    }

    /// Update the timestamp's of all the labels that are associated with Pubkey
    pub(crate) fn update_record_timestamp(&mut self, pubkey: &Pubkey, now: u64) {
        // It suffices to only overwrite the origin's timestamp since that is
        // used when purging old values. If the origin does not exist in the
        // table, fallback to exhaustive update on all associated records.
        let origin = CrdsValueLabel::ContactInfo(*pubkey);
        if let Some(origin) = self.table.get_mut(&origin) {
            if origin.local_timestamp < now {
                origin.local_timestamp = now;
            }
        } else if let Some(indices) = self.records.get(pubkey) {
            for index in indices {
                let entry = self.table.index_mut(*index);
                if entry.local_timestamp < now {
                    entry.local_timestamp = now;
                }
            }
        }
    }

    /// Find all the keys that are older or equal to the timeout.
    /// * timeouts - Pubkey specific timeouts with Pubkey::default() as the default timeout.
    pub fn find_old_labels(
        &self,
        thread_pool: &ThreadPool,
        now: u64,
        timeouts: &CrdsTimeouts,
    ) -> Vec<CrdsValueLabel> {
        // Given an index of all crd values associated with a pubkey,
        // returns crds labels of old values to be evicted.
        let evict = |pubkey, index: &IndexSet<usize>| {
            let timeout = timeouts[pubkey];
            // If the origin's contact-info hasn't expired yet then preserve
            // all associated values.
            let origin = CrdsValueLabel::ContactInfo(*pubkey);
            if let Some(origin) = self.table.get(&origin) {
                if origin
                    .value
                    .wallclock()
                    .min(origin.local_timestamp)
                    .saturating_add(timeout)
                    > now
                {
                    return vec![];
                }
            }
            // Otherwise check each value's timestamp individually.
            index
                .into_iter()
                .map(|&ix| self.table.get_index(ix).unwrap())
                .filter(|(_, entry)| {
                    entry
                        .value
                        .wallclock()
                        .min(entry.local_timestamp)
                        .saturating_add(timeout)
                        <= now
                })
                .map(|(label, _)| label)
                .cloned()
                .collect::<Vec<_>>()
        };
        thread_pool.install(|| {
            self.records
                .par_iter()
                .flat_map(|(pubkey, index)| evict(pubkey, index))
                .collect()
        })
    }

    pub fn remove(&mut self, key: &CrdsValueLabel, now: u64) {
        let Some((index, _ /*label*/, value)) = self.table.swap_remove_full(key) else {
            return;
        };
        self.purged.push_back((*value.value.hash(), now));
        self.shards.remove(index, &value);
        match value.value.data() {
            CrdsData::ContactInfo(_) => {
                self.nodes.swap_remove(&index);
            }
            CrdsData::Vote(_, _) => {
                self.votes.remove(&value.ordinal);
            }
            CrdsData::EpochSlots(_, _) => {
                self.epoch_slots.remove(&value.ordinal);
            }
            CrdsData::DuplicateShred(_, _) => {
                self.duplicate_shreds.remove(&value.ordinal);
            }
            _ => (),
        }
        self.entries.remove(&value.ordinal);
        // Remove the index from records associated with the value's pubkey.
        let pubkey = value.value.pubkey();
        let hash_map::Entry::Occupied(mut records_entry) = self.records.entry(pubkey) else {
            panic!("this should not happen!");
        };
        records_entry.get_mut().swap_remove(&index);
        if records_entry.get().is_empty() {
            records_entry.remove();
            self.shred_versions.remove(&pubkey);
        }
        // If index == self.table.len(), then the removed entry was the last
        // entry in the table, in which case no other keys were modified.
        // Otherwise, the previously last element in the table is now moved to
        // the 'index' position; and so shards and nodes need to be updated
        // accordingly.
        let size = self.table.len();
        if index < size {
            let value = self.table.index(index);
            self.shards.remove(size, value);
            self.shards.insert(index, value);
            match value.value.data() {
                CrdsData::ContactInfo(_) => {
                    self.nodes.swap_remove(&size);
                    self.nodes.insert(index);
                }
                CrdsData::Vote(_, _) => {
                    self.votes.insert(value.ordinal, index);
                }
                CrdsData::EpochSlots(_, _) => {
                    self.epoch_slots.insert(value.ordinal, index);
                }
                CrdsData::DuplicateShred(_, _) => {
                    self.duplicate_shreds.insert(value.ordinal, index);
                }
                _ => (),
            };
            self.entries.insert(value.ordinal, index);
            let pubkey = value.value.pubkey();
            let records = self.records.get_mut(&pubkey).unwrap();
            records.swap_remove(&size);
            records.insert(index);
        }
    }

    /// Returns true if the number of unique pubkeys in the table exceeds the
    /// given capacity (plus some margin).
    /// Allows skipping unnecessary calls to trim without obtaining a write
    /// lock on gossip.
    pub(crate) fn should_trim(&self, cap: usize) -> bool {
        // Allow 10% overshoot so that the computation cost is amortized down.
        10 * self.records.len() > 11 * cap
    }

    /// Trims the table by dropping all values associated with the pubkeys with
    /// the lowest stake, so that the number of unique pubkeys are bounded.
    pub(crate) fn trim(
        &mut self,
        cap: usize, // Capacity hint for number of unique pubkeys.
        // Set of pubkeys to never drop.
        // e.g. known validators, self pubkey, ...
        keep: &[Pubkey],
        stakes: &HashMap<Pubkey, u64>,
        now: u64,
    ) -> Result</*num purged:*/ usize, CrdsError> {
        if self.should_trim(cap) {
            let size = self.records.len().saturating_sub(cap);
            self.drop(size, keep, stakes, now)
        } else {
            Ok(0)
        }
    }

    // Drops 'size' many pubkeys with the lowest stake.
    fn drop(
        &mut self,
        size: usize,
        keep: &[Pubkey],
        stakes: &HashMap<Pubkey, u64>,
        now: u64,
    ) -> Result</*num purged:*/ usize, CrdsError> {
        if stakes.values().all(|&stake| stake == 0) {
            return Err(CrdsError::UnknownStakes);
        }
        let mut keys: Vec<_> = self
            .records
            .keys()
            .map(|k| (stakes.get(k).copied().unwrap_or_default(), *k))
            .collect();
        if size < keys.len() {
            keys.select_nth_unstable(size);
        }
        let keys: Vec<_> = keys
            .into_iter()
            .take(size)
            .map(|(_, k)| k)
            .filter(|k| !keep.contains(k))
            .flat_map(|k| &self.records[&k])
            .map(|k| self.table.get_index(*k).unwrap().0.clone())
            .collect();
        for key in &keys {
            self.remove(key, now);
        }
        Ok(keys.len())
    }

    pub(crate) fn take_stats(&self) -> CrdsStats {
        std::mem::take(&mut self.stats.lock().unwrap())
    }
}

impl Default for CrdsDataStats {
    fn default() -> Self {
        Self {
            counts: CrdsCountsArray::default(),
            fails: CrdsCountsArray::default(),
            votes: LruCache::new(VOTE_SLOTS_METRICS_CAP),
        }
    }
}

impl CrdsDataStats {
    fn record_insert(&mut self, entry: &VersionedCrdsValue, route: GossipRoute) {
        self.counts[Self::ordinal(entry)] += 1;
        if let CrdsData::Vote(_, vote) = entry.value.data() {
            if let Some(slot) = vote.slot() {
                let num_nodes = self.votes.get(&slot).copied().unwrap_or_default();
                self.votes.put(slot, num_nodes + 1);
            }
        }

        let GossipRoute::PushMessage(from) = route else {
            return;
        };

        if should_report_message_signature(entry.value.signature()) {
            datapoint_info!(
                "gossip_crds_sample",
                (
                    "origin",
                    entry.value.pubkey().to_string().get(..8),
                    Option<String>
                ),
                (
                    "signature",
                    entry.value.signature().to_string().get(..8),
                    Option<String>
                ),
                (
                    "from",
                    from.to_string().get(..8),
                    Option<String>
                )
            );
        }
    }

    fn record_fail(&mut self, entry: &VersionedCrdsValue) {
        self.fails[Self::ordinal(entry)] += 1;
    }

    fn ordinal(entry: &VersionedCrdsValue) -> usize {
        match entry.value.data() {
            CrdsData::LegacyContactInfo(_) => 0,
            CrdsData::Vote(_, _) => 1,
            CrdsData::LowestSlot(_, _) => 2,
            CrdsData::LegacySnapshotHashes(_) => 3,
            CrdsData::AccountsHashes(_) => 4,
            CrdsData::EpochSlots(_, _) => 5,
            CrdsData::LegacyVersion(_) => 6,
            CrdsData::Version(_) => 7,
            CrdsData::NodeInstance(_) => 8,
            CrdsData::DuplicateShred(_, _) => 9,
            CrdsData::SnapshotHashes(_) => 10,
            CrdsData::ContactInfo(_) => 11,
            CrdsData::RestartLastVotedForkSlots(_) => 12,
            CrdsData::RestartHeaviestFork(_) => 13,
            // Update CrdsCountsArray if new items are added here.
        }
    }
}

impl CrdsStats {
    /// 记录插入
    fn record_insert(&mut self, entry: &VersionedCrdsValue, route: GossipRoute) {
        match route {
            GossipRoute::LocalMessage => (),
            GossipRoute::PullRequest => (),
            GossipRoute::PushMessage(_) => self.push.record_insert(entry, route),
            GossipRoute::PullResponse => self.pull.record_insert(entry, route),
        }
    }

    fn record_fail(&mut self, entry: &VersionedCrdsValue, route: GossipRoute) {
        match route {
            GossipRoute::LocalMessage => (),
            GossipRoute::PullRequest => (),
            GossipRoute::PushMessage(_) => self.push.record_fail(entry),
            GossipRoute::PullResponse => self.pull.record_fail(entry),
        }
    }
}

/// check if first SIGNATURE_SAMPLE_LEADING_ZEROS bits of signature are 0
#[inline]
fn should_report_message_signature(signature: &Signature) -> bool {
    let Some(Ok(bytes)) = signature.as_ref().get(..8).map(<[u8; 8]>::try_from) else {
        return false;
    };
    u64::from_le_bytes(bytes).trailing_zeros() >= SIGNATURE_SAMPLE_LEADING_ZEROS
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::crds_data::{new_rand_timestamp, AccountsHashes, NodeInstance},
        rand::{thread_rng, Rng, SeedableRng},
        rand_chacha::ChaChaRng,
        rayon::ThreadPoolBuilder,
        solana_sdk::{
            signature::{Keypair, Signer},
            timing::timestamp,
        },
        std::{collections::HashSet, iter::repeat_with, net::Ipv4Addr, time::Duration},
    };

    #[test]
    fn test_insert() {
        let mut crds = Crds::default();
        let val = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::default()));
        assert_eq!(
            crds.insert(val.clone(), 0, GossipRoute::LocalMessage),
            Ok(())
        );
        assert_eq!(crds.table.len(), 1);
        assert!(crds.table.contains_key(&val.label()));
        assert_eq!(crds.table[&val.label()].local_timestamp, 0);
    }
    #[test]
    fn test_update_old() {
        let mut crds = Crds::default();
        let val = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::default()));
        assert_eq!(
            crds.insert(val.clone(), 0, GossipRoute::LocalMessage),
            Ok(())
        );
        assert_eq!(
            crds.insert(val.clone(), 1, GossipRoute::LocalMessage),
            Err(CrdsError::InsertFailed)
        );
        assert!(crds.purged.is_empty());
        assert_eq!(crds.table[&val.label()].local_timestamp, 0);
    }
    #[test]
    fn test_update_new() {
        let mut crds = Crds::default();
        let original = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::new_localhost(
            &Pubkey::default(),
            0,
        )));
        let value_hash = *original.hash();
        assert_matches!(crds.insert(original, 0, GossipRoute::LocalMessage), Ok(()));
        let val = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::new_localhost(
            &Pubkey::default(),
            1,
        )));
        assert_eq!(
            crds.insert(val.clone(), 1, GossipRoute::LocalMessage),
            Ok(())
        );
        assert_eq!(*crds.purged.back().unwrap(), (value_hash, 1));
        assert_eq!(crds.table[&val.label()].local_timestamp, 1);
    }
    #[test]
    fn test_update_timestamp() {
        let mut crds = Crds::default();
        let val1 = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::new_localhost(
            &Pubkey::default(),
            0,
        )));
        let val1_hash = *val1.hash();
        assert_eq!(
            crds.insert(val1.clone(), 0, GossipRoute::LocalMessage),
            Ok(())
        );
        assert_eq!(crds.table[&val1.label()].local_timestamp, 0);
        assert_eq!(crds.table[&val1.label()].ordinal, 0);

        // `val2` is expected to overwrite `val1` based on the `wallclock` value.
        let val2 = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::new_localhost(
            &Pubkey::default(),
            1,
        )));
        assert_eq!(val2.label().pubkey(), val1.label().pubkey());
        assert_eq!(
            crds.insert(val2.clone(), 1, GossipRoute::LocalMessage),
            Ok(())
        );
        assert_eq!(*crds.purged.back().unwrap(), (val1_hash, 1));

        assert_eq!(crds.table[&val2.label()].local_timestamp, 1);
        assert_eq!(crds.table[&val2.label()].ordinal, 1);

        crds.update_record_timestamp(&val2.label().pubkey(), 2);
        assert_eq!(crds.table[&val2.label()].local_timestamp, 2);
        assert_eq!(crds.table[&val2.label()].ordinal, 1);

        crds.update_record_timestamp(&val2.label().pubkey(), 1);
        assert_eq!(crds.table[&val2.label()].local_timestamp, 2);
        assert_eq!(crds.table[&val2.label()].ordinal, 1);
    }

    #[test]
    fn test_upsert_node_instance() {
        const SEED: [u8; 32] = [0x42; 32];
        let mut rng = ChaChaRng::from_seed(SEED);
        fn make_crds_value(node: NodeInstance) -> CrdsValue {
            CrdsValue::new_unsigned(CrdsData::NodeInstance(node))
        }
        let now = 1_620_838_767_000;
        let mut crds = Crds::default();
        let pubkey = Pubkey::new_unique();
        let node = NodeInstance::new(&mut rng, pubkey, now);
        let node = make_crds_value(node);
        assert_eq!(crds.insert(node, now, GossipRoute::LocalMessage), Ok(()));
        // A node-instance with a different key should insert fine even with
        // older timestamps.
        let other = NodeInstance::new(&mut rng, Pubkey::new_unique(), now - 1);
        let other = make_crds_value(other);
        assert_eq!(crds.insert(other, now, GossipRoute::LocalMessage), Ok(()));
        // A node-instance with older timestamp should fail to insert, even if
        // the wallclock is more recent.
        let other = NodeInstance::new(&mut rng, pubkey, now - 1);
        let other = other.with_wallclock(now + 1);
        let other = make_crds_value(other);
        let value_hash = *other.hash();
        assert_eq!(
            crds.insert(other, now, GossipRoute::LocalMessage),
            Err(CrdsError::InsertFailed)
        );
        assert_eq!(*crds.purged.back().unwrap(), (value_hash, now));
        // A node instance with the same timestamp should insert only if the
        // random token is larger.
        let mut num_overrides = 0;
        for _ in 0..100 {
            let other = NodeInstance::new(&mut rng, pubkey, now);
            let other = make_crds_value(other);
            let value_hash = *other.hash();
            match crds.insert(other, now, GossipRoute::LocalMessage) {
                Ok(()) => num_overrides += 1,
                Err(CrdsError::InsertFailed) => {
                    assert_eq!(*crds.purged.back().unwrap(), (value_hash, now))
                }
                _ => panic!(),
            }
        }
        assert_eq!(num_overrides, 5);
        // A node instance with larger timestamp should insert regardless of
        // its token value.
        for k in 1..10 {
            let other = NodeInstance::new(&mut rng, pubkey, now + k);
            let other = other.with_wallclock(now - 1);
            let other = make_crds_value(other);
            assert_matches!(crds.insert(other, now, GossipRoute::LocalMessage), Ok(()));
        }
    }

    #[test]
    fn test_find_old_records_default() {
        let thread_pool = ThreadPoolBuilder::new().build().unwrap();
        let mut crds = Crds::default();
        let val = {
            let node = ContactInfo::new_localhost(&Pubkey::default(), /*now:*/ 1);
            CrdsValue::new_unsigned(CrdsData::ContactInfo(node))
        };
        assert_eq!(
            crds.insert(val.clone(), 1, GossipRoute::LocalMessage),
            Ok(())
        );
        let pubkey = Pubkey::new_unique();
        let stakes = HashMap::from([(Pubkey::new_unique(), 1u64)]);
        let epoch_duration = Duration::from_secs(48 * 3600);
        let timeouts = CrdsTimeouts::new(
            pubkey,
            0u64, // default_timeout,
            epoch_duration,
            &stakes,
        );
        assert!(crds.find_old_labels(&thread_pool, 0, &timeouts).is_empty());
        let timeouts = CrdsTimeouts::new(
            pubkey,
            1u64, // default_timeout,
            epoch_duration,
            &stakes,
        );
        assert_eq!(
            crds.find_old_labels(&thread_pool, 2, &timeouts),
            vec![val.label()]
        );
        let timeouts = CrdsTimeouts::new(
            pubkey,
            2u64, // default_timeout,
            epoch_duration,
            &stakes,
        );
        assert_eq!(
            crds.find_old_labels(&thread_pool, 4, &timeouts),
            vec![val.label()]
        );
    }
    #[test]
    fn test_find_old_records_with_override() {
        let thread_pool = ThreadPoolBuilder::new().build().unwrap();
        let mut rng = thread_rng();
        let mut crds = Crds::default();
        let val = CrdsValue::new_rand(&mut rng, None);
        let mut stakes = HashMap::from([(Pubkey::new_unique(), 1u64)]);
        let timeouts = CrdsTimeouts::new(
            Pubkey::new_unique(),
            3,                              // default_timeout
            Duration::from_secs(48 * 3600), // epoch_duration
            &stakes,
        );
        assert_eq!(
            crds.insert(val.clone(), 0, GossipRoute::LocalMessage),
            Ok(())
        );
        assert!(crds.find_old_labels(&thread_pool, 2, &timeouts).is_empty());
        stakes.insert(val.pubkey(), 1u64);
        let timeouts = CrdsTimeouts::new(
            Pubkey::new_unique(),
            1,                        // default_timeout
            Duration::from_millis(1), // epoch_duration
            &stakes,
        );
        assert_eq!(
            crds.find_old_labels(&thread_pool, 2, &timeouts),
            vec![val.label()]
        );
        let timeouts = CrdsTimeouts::new(
            Pubkey::new_unique(),
            3,                              // default_timeout
            Duration::from_secs(48 * 3600), // epoch_duration
            &stakes,
        );
        assert!(crds.find_old_labels(&thread_pool, 2, &timeouts).is_empty());
        let timeouts = CrdsTimeouts::new(
            Pubkey::new_unique(),
            1,                              // default_timeout
            Duration::from_secs(48 * 3600), // epoch_duration
            &stakes,
        );
        assert!(crds.find_old_labels(&thread_pool, 2, &timeouts).is_empty());
        stakes.remove(&val.pubkey());
        let timeouts = CrdsTimeouts::new(
            Pubkey::new_unique(),
            1,                              // default_timeout
            Duration::from_secs(48 * 3600), // epoch_duration
            &stakes,
        );
        assert_eq!(
            crds.find_old_labels(&thread_pool, 2, &timeouts),
            vec![val.label()]
        );
    }

    #[test]
    fn test_remove_default() {
        let thread_pool = ThreadPoolBuilder::new().build().unwrap();
        let mut crds = Crds::default();
        let val = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::default()));
        assert_matches!(
            crds.insert(val.clone(), 1, GossipRoute::LocalMessage),
            Ok(_)
        );
        let stakes = HashMap::from([(Pubkey::new_unique(), 1u64)]);
        let timeouts = CrdsTimeouts::new(
            Pubkey::new_unique(),
            1,                              // default_timeout
            Duration::from_secs(48 * 3600), // epoch_duration
            &stakes,
        );
        assert_eq!(
            crds.find_old_labels(&thread_pool, 2, &timeouts),
            vec![val.label()]
        );
        crds.remove(&val.label(), /*now=*/ 0);
        assert!(crds.find_old_labels(&thread_pool, 2, &timeouts).is_empty());
    }
    #[test]
    fn test_find_old_records_staked() {
        let thread_pool = ThreadPoolBuilder::new().build().unwrap();
        let mut crds = Crds::default();
        let val = {
            let node = ContactInfo::new_localhost(&Pubkey::default(), /*now:*/ 1);
            CrdsValue::new_unsigned(CrdsData::ContactInfo(node))
        };
        assert_eq!(
            crds.insert(val.clone(), 1, GossipRoute::LocalMessage),
            Ok(())
        );
        let mut stakes = HashMap::from([(Pubkey::new_unique(), 1u64)]);
        let timeouts = CrdsTimeouts::new(
            Pubkey::new_unique(),
            0,                              // default_timeout
            Duration::from_secs(48 * 3600), // epoch_duration
            &stakes,
        );
        //now < timestamp
        assert!(crds.find_old_labels(&thread_pool, 0, &timeouts).is_empty());

        //pubkey shouldn't expire since its timeout is MAX
        stakes.insert(val.pubkey(), 1u64);
        let timeouts = CrdsTimeouts::new(
            Pubkey::new_unique(),
            0,                              // default_timeout
            Duration::from_secs(48 * 3600), // epoch_duration
            &stakes,
        );
        assert!(crds.find_old_labels(&thread_pool, 2, &timeouts).is_empty());

        let timeouts = CrdsTimeouts::new(
            Pubkey::new_unique(),
            0,                        // default_timeout
            Duration::from_millis(2), // epoch_duration
            &stakes,
        );
        assert!(crds.find_old_labels(&thread_pool, 2, &timeouts).is_empty());
        assert_eq!(
            crds.find_old_labels(&thread_pool, 3, &timeouts),
            vec![val.label()]
        );
    }

    #[test]
    fn test_crds_shards() {
        fn check_crds_shards(crds: &Crds) {
            crds.shards
                .check(&crds.table.values().cloned().collect::<Vec<_>>());
        }

        let mut crds = Crds::default();
        let keypairs: Vec<_> = std::iter::repeat_with(Keypair::new).take(256).collect();
        let mut rng = thread_rng();
        let mut num_inserts = 0;
        for _ in 0..4096 {
            let keypair = &keypairs[rng.gen_range(0..keypairs.len())];
            let value = CrdsValue::new_rand(&mut rng, Some(keypair));
            let local_timestamp = new_rand_timestamp(&mut rng);
            if let Ok(()) = crds.insert(value, local_timestamp, GossipRoute::LocalMessage) {
                num_inserts += 1;
                check_crds_shards(&crds);
            }
        }
        assert_eq!(num_inserts, crds.cursor.0 as usize);
        assert!(num_inserts > 700);
        assert!(crds.num_purged() > 500);
        assert_eq!(crds.num_purged() + crds.table.len(), 4096);
        assert!(crds.table.len() > 200);
        assert!(num_inserts > crds.table.len());
        check_crds_shards(&crds);
        // Remove values one by one and assert that shards stay valid.
        while !crds.table.is_empty() {
            let index = rng.gen_range(0..crds.table.len());
            let key = crds.table.get_index(index).unwrap().0.clone();
            crds.remove(&key, /*now=*/ 0);
            check_crds_shards(&crds);
        }
    }

    fn check_crds_value_indices<R: rand::Rng>(
        rng: &mut R,
        crds: &Crds,
    ) -> (
        usize, // number of nodes
        usize, // number of votes
        usize, // number of epoch slots
    ) {
        let size = crds.table.len();
        let since = if size == 0 || rng.gen() {
            rng.gen_range(0..crds.cursor.0 + 1)
        } else {
            crds.table[rng.gen_range(0..size)].ordinal
        };
        let num_epoch_slots = crds
            .table
            .values()
            .filter(|v| v.ordinal >= since)
            .filter(|v| matches!(v.value.data(), CrdsData::EpochSlots(_, _)))
            .count();
        let mut cursor = Cursor(since);
        assert_eq!(num_epoch_slots, crds.get_epoch_slots(&mut cursor).count());
        assert_eq!(
            cursor.0,
            crds.epoch_slots
                .iter()
                .last()
                .map(|(k, _)| k + 1)
                .unwrap_or_default()
                .max(since)
        );
        for value in crds.get_epoch_slots(&mut Cursor(since)) {
            assert!(value.ordinal >= since);
            assert_matches!(value.value.data(), CrdsData::EpochSlots(_, _));
        }
        let num_votes = crds
            .table
            .values()
            .filter(|v| v.ordinal >= since)
            .filter(|v| matches!(v.value.data(), CrdsData::Vote(_, _)))
            .count();
        let mut cursor = Cursor(since);
        assert_eq!(num_votes, crds.get_votes(&mut cursor).count());
        assert_eq!(
            cursor.0,
            crds.table
                .values()
                .filter(|v| matches!(v.value.data(), CrdsData::Vote(_, _)))
                .map(|v| v.ordinal)
                .max()
                .map(|k| k + 1)
                .unwrap_or_default()
                .max(since)
        );
        for value in crds.get_votes(&mut Cursor(since)) {
            assert!(value.ordinal >= since);
            assert_matches!(value.value.data(), CrdsData::Vote(_, _));
        }
        let num_entries = crds
            .table
            .values()
            .filter(|value| value.ordinal >= since)
            .count();
        let mut cursor = Cursor(since);
        assert_eq!(num_entries, crds.get_entries(&mut cursor).count());
        assert_eq!(
            cursor.0,
            crds.entries
                .iter()
                .last()
                .map(|(k, _)| k + 1)
                .unwrap_or_default()
                .max(since)
        );
        for value in crds.get_entries(&mut Cursor(since)) {
            assert!(value.ordinal >= since);
        }
        let num_nodes = crds
            .table
            .values()
            .filter(|v| matches!(v.value.data(), CrdsData::ContactInfo(_)))
            .count();
        let num_votes = crds
            .table
            .values()
            .filter(|v| matches!(v.value.data(), CrdsData::Vote(_, _)))
            .count();
        let num_epoch_slots = crds
            .table
            .values()
            .filter(|v| matches!(v.value.data(), CrdsData::EpochSlots(_, _)))
            .count();
        assert_eq!(
            crds.table.len(),
            crds.get_entries(&mut Cursor::default()).count()
        );
        assert_eq!(num_nodes, crds.get_nodes_contact_info().count());
        assert_eq!(num_votes, crds.get_votes(&mut Cursor::default()).count());
        assert_eq!(
            num_epoch_slots,
            crds.get_epoch_slots(&mut Cursor::default()).count()
        );
        for vote in crds.get_votes(&mut Cursor::default()) {
            assert_matches!(vote.value.data(), CrdsData::Vote(_, _));
        }
        for epoch_slots in crds.get_epoch_slots(&mut Cursor::default()) {
            assert_matches!(epoch_slots.value.data(), CrdsData::EpochSlots(_, _));
        }
        (num_nodes, num_votes, num_epoch_slots)
    }

    #[test]
    fn test_crds_value_indices() {
        let mut rng = thread_rng();
        let keypairs: Vec<_> = repeat_with(Keypair::new).take(128).collect();
        let mut crds = Crds::default();
        let mut num_inserts = 0;
        for k in 0..4096 {
            let keypair = &keypairs[rng.gen_range(0..keypairs.len())];
            let value = CrdsValue::new_rand(&mut rng, Some(keypair));
            let local_timestamp = new_rand_timestamp(&mut rng);
            if let Ok(()) = crds.insert(value, local_timestamp, GossipRoute::LocalMessage) {
                num_inserts += 1;
            }
            if k % 16 == 0 {
                check_crds_value_indices(&mut rng, &crds);
            }
        }
        assert_eq!(num_inserts, crds.cursor.0 as usize);
        assert!(num_inserts > 700);
        assert!(crds.num_purged() > 500);
        assert!(crds.table.len() > 200);
        assert_eq!(crds.num_purged() + crds.table.len(), 4096);
        assert!(num_inserts > crds.table.len());
        let (num_nodes, num_votes, num_epoch_slots) = check_crds_value_indices(&mut rng, &crds);
        assert!(num_nodes * 3 < crds.table.len());
        assert!(num_nodes > 100, "num nodes: {num_nodes}");
        assert!(num_votes > 100, "num votes: {num_votes}");
        assert!(num_epoch_slots > 100, "num epoch slots: {num_epoch_slots}");
        // Remove values one by one and assert that nodes indices stay valid.
        while !crds.table.is_empty() {
            let index = rng.gen_range(0..crds.table.len());
            let key = crds.table.get_index(index).unwrap().0.clone();
            crds.remove(&key, /*now=*/ 0);
            if crds.table.len() % 16 == 0 {
                check_crds_value_indices(&mut rng, &crds);
            }
        }
    }

    #[test]
    fn test_crds_records() {
        fn check_crds_records(crds: &Crds) {
            assert_eq!(
                crds.table.len(),
                crds.records.values().map(IndexSet::len).sum::<usize>()
            );
            for (pubkey, indices) in &crds.records {
                for index in indices {
                    let value = crds.table.index(*index);
                    assert_eq!(*pubkey, value.value.pubkey());
                }
            }
        }
        let mut rng = thread_rng();
        let keypairs: Vec<_> = repeat_with(Keypair::new).take(128).collect();
        let mut crds = Crds::default();
        for k in 0..4096 {
            let keypair = &keypairs[rng.gen_range(0..keypairs.len())];
            let value = CrdsValue::new_rand(&mut rng, Some(keypair));
            let local_timestamp = new_rand_timestamp(&mut rng);
            let _ = crds.insert(value, local_timestamp, GossipRoute::LocalMessage);
            if k % 64 == 0 {
                check_crds_records(&crds);
            }
        }
        assert!(crds.records.len() > 96);
        assert!(crds.records.len() <= keypairs.len());
        // Remove values one by one and assert that records stay valid.
        while !crds.table.is_empty() {
            let index = rng.gen_range(0..crds.table.len());
            let key = crds.table.get_index(index).unwrap().0.clone();
            crds.remove(&key, /*now=*/ 0);
            if crds.table.len() % 64 == 0 {
                check_crds_records(&crds);
            }
        }
        assert!(crds.records.is_empty());
    }

    #[test]
    fn test_get_shred_version() {
        let mut rng = rand::thread_rng();
        let pubkey = Pubkey::new_unique();
        let mut crds = Crds::default();
        assert_eq!(crds.get_shred_version(&pubkey), None);
        // Initial insertion of a node with shred version:
        let mut node = ContactInfo::new_rand(&mut rng, Some(pubkey));
        let wallclock = node.wallclock();
        node.set_shred_version(42);
        {
            let node = CrdsData::ContactInfo(node.clone());
            let node = CrdsValue::new_unsigned(node);
            assert_eq!(
                crds.insert(node, timestamp(), GossipRoute::LocalMessage),
                Ok(())
            );
        }
        assert_eq!(crds.get_shred_version(&pubkey), Some(42));
        // An outdated  value should not update shred-version:
        let mut node = node.clone();
        node.set_wallclock(wallclock - 1); // outdated.
        node.set_shred_version(8);
        let node = CrdsData::ContactInfo(node);
        let node = CrdsValue::new_unsigned(node);
        assert_eq!(
            crds.insert(node, timestamp(), GossipRoute::LocalMessage),
            Err(CrdsError::InsertFailed)
        );
        assert_eq!(crds.get_shred_version(&pubkey), Some(42));
        // Update shred version:
        let mut node = ContactInfo::new_rand(&mut rng, Some(pubkey));
        node.set_wallclock(wallclock + 1); // so that it overrides the prev one.
        node.set_shred_version(8);
        let node = CrdsData::ContactInfo(node);
        let node = CrdsValue::new_unsigned(node);
        assert_eq!(
            crds.insert(node, timestamp(), GossipRoute::LocalMessage),
            Ok(())
        );
        assert_eq!(crds.get_shred_version(&pubkey), Some(8));
        // Add other crds values with the same pubkey.
        let val = AccountsHashes::new_rand(&mut rng, Some(pubkey));
        let val = CrdsData::AccountsHashes(val);
        let val = CrdsValue::new_unsigned(val);
        assert_eq!(
            crds.insert(val, timestamp(), GossipRoute::LocalMessage),
            Ok(())
        );
        assert_eq!(crds.get_shred_version(&pubkey), Some(8));
        // Remove contact-info. Shred version should stay there since there
        // are still values associated with the pubkey.
        crds.remove(&CrdsValueLabel::ContactInfo(pubkey), timestamp());
        assert_eq!(crds.get::<&ContactInfo>(pubkey), None);
        assert_eq!(crds.get_shred_version(&pubkey), Some(8));
        // Remove the remaining entry with the same pubkey.
        crds.remove(&CrdsValueLabel::AccountsHashes(pubkey), timestamp());
        assert_eq!(crds.get_records(&pubkey).count(), 0);
        assert_eq!(crds.get_shred_version(&pubkey), None);
    }

    #[test]
    fn test_drop() {
        fn num_unique_pubkeys<'a, I>(values: I) -> usize
        where
            I: IntoIterator<Item = &'a VersionedCrdsValue>,
        {
            values
                .into_iter()
                .map(|v| v.value.pubkey())
                .collect::<HashSet<_>>()
                .len()
        }
        let mut rng = thread_rng();
        let keypairs: Vec<_> = repeat_with(Keypair::new).take(64).collect();
        let stakes = keypairs
            .iter()
            .map(|k| (k.pubkey(), rng.gen_range(0..1000)))
            .collect();
        let mut crds = Crds::default();
        for _ in 0..2048 {
            let keypair = &keypairs[rng.gen_range(0..keypairs.len())];
            let value = CrdsValue::new_rand(&mut rng, Some(keypair));
            let local_timestamp = new_rand_timestamp(&mut rng);
            let _ = crds.insert(value, local_timestamp, GossipRoute::LocalMessage);
        }
        let num_values = crds.table.len();
        let num_pubkeys = num_unique_pubkeys(crds.table.values());
        assert!(!crds.should_trim(num_pubkeys));
        assert!(crds.should_trim(num_pubkeys * 5 / 6));
        let values: Vec<_> = crds.table.values().cloned().collect();
        crds.drop(16, &[], &stakes, /*now=*/ 0).unwrap();
        let purged: Vec<_> = {
            let purged: HashSet<_> = crds.purged.iter().map(|(hash, _)| hash).copied().collect();
            values
                .into_iter()
                .filter(|v| purged.contains(v.value.hash()))
                .collect()
        };
        assert_eq!(purged.len() + crds.table.len(), num_values);
        assert_eq!(num_unique_pubkeys(&purged), 16);
        assert_eq!(num_unique_pubkeys(crds.table.values()), num_pubkeys - 16);
        let attach_stake = |v: &VersionedCrdsValue| {
            let pk = v.value.pubkey();
            (stakes[&pk], pk)
        };
        assert!(
            purged.iter().map(attach_stake).max().unwrap()
                < crds.table.values().map(attach_stake).min().unwrap()
        );
        let purged = purged
            .into_iter()
            .map(|v| v.value.pubkey())
            .collect::<HashSet<_>>();
        for (k, v) in crds.table {
            assert!(!purged.contains(&k.pubkey()));
            assert!(!purged.contains(&v.value.pubkey()));
        }
    }

    #[test]
    fn test_remove_staked() {
        let thread_pool = ThreadPoolBuilder::new().build().unwrap();
        let mut crds = Crds::default();
        let val = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::default()));
        assert_matches!(
            crds.insert(val.clone(), 1, GossipRoute::LocalMessage),
            Ok(_)
        );
        let stakes = HashMap::from([(Pubkey::new_unique(), 1u64)]);
        let timeouts = CrdsTimeouts::new(
            Pubkey::new_unique(),
            1,                        // default_timeout
            Duration::from_millis(1), // epoch_duration
            &stakes,
        );
        assert_eq!(
            crds.find_old_labels(&thread_pool, 2, &timeouts),
            vec![val.label()]
        );
        crds.remove(&val.label(), /*now=*/ 0);
        assert!(crds.find_old_labels(&thread_pool, 2, &timeouts).is_empty());
    }

    #[test]
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn test_equal() {
        let val = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::default()));
        let v1 =
            VersionedCrdsValue::new(val.clone(), Cursor::default(), 1, GossipRoute::LocalMessage);
        let v2 = VersionedCrdsValue::new(val, Cursor::default(), 1, GossipRoute::LocalMessage);
        assert_eq!(v1, v2);
        assert!(!(v1 != v2));
        assert!(!overrides(&v1.value, &v2));
        assert!(!overrides(&v2.value, &v1));
    }
    #[test]
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn test_hash_order() {
        let mut node = ContactInfo::new_localhost(&Pubkey::default(), 0);
        let v1 = VersionedCrdsValue::new(
            CrdsValue::new_unsigned(CrdsData::ContactInfo(node.clone())),
            Cursor::default(),
            1, // local_timestamp
            GossipRoute::LocalMessage,
        );
        let v2 = VersionedCrdsValue::new(
            {
                node.set_rpc((Ipv4Addr::LOCALHOST, 1244)).unwrap();
                CrdsValue::new_unsigned(CrdsData::ContactInfo(node))
            },
            Cursor::default(),
            1, // local_timestamp
            GossipRoute::LocalMessage,
        );

        assert_eq!(v1.value.label(), v2.value.label());
        assert_eq!(v1.value.wallclock(), v2.value.wallclock());
        assert_ne!(v1.value.hash(), v2.value.hash());
        assert!(v1 != v2);
        assert!(!(v1 == v2));
        if v1.value.hash() > v2.value.hash() {
            assert!(overrides(&v1.value, &v2));
            assert!(!overrides(&v2.value, &v1));
        } else {
            assert!(overrides(&v2.value, &v1));
            assert!(!overrides(&v1.value, &v2));
        }
    }
    #[test]
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn test_wallclock_order() {
        let mut node = ContactInfo::new_localhost(&Pubkey::default(), 1);
        let v1 = VersionedCrdsValue::new(
            CrdsValue::new_unsigned(CrdsData::ContactInfo(node.clone())),
            Cursor::default(),
            1, // local_timestamp
            GossipRoute::LocalMessage,
        );
        node.set_wallclock(0);
        let v2 = VersionedCrdsValue::new(
            CrdsValue::new_unsigned(CrdsData::ContactInfo(node)),
            Cursor::default(),
            1, // local_timestamp
            GossipRoute::LocalMessage,
        );
        assert_eq!(v1.value.label(), v2.value.label());
        assert!(overrides(&v1.value, &v2));
        assert!(!overrides(&v2.value, &v1));
        assert!(v1 != v2);
        assert!(!(v1 == v2));
    }
    #[test]
    #[should_panic(expected = "labels mismatch!")]
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    fn test_label_order() {
        let v1 = VersionedCrdsValue::new(
            CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::new_localhost(
                &solana_sdk::pubkey::new_rand(),
                0,
            ))),
            Cursor::default(),
            1, // local_timestamp
            GossipRoute::LocalMessage,
        );
        let v2 = VersionedCrdsValue::new(
            CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::new_localhost(
                &solana_sdk::pubkey::new_rand(),
                0,
            ))),
            Cursor::default(),
            1, // local_timestamp
            GossipRoute::LocalMessage,
        );
        assert_ne!(v1, v2);
        assert!(!(v1 == v2));
        assert!(!overrides(&v2.value, &v1));
    }
}
