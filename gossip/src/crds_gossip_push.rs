//! Crds Gossip Push overlay.
//!
//! This module is used to propagate recently created CrdsValues across the network
//! Eager push strategy is based on [Plumtree].
//!
//! [Plumtree]: http://asc.di.fct.unl.pt/~jleitao/pdf/srds07-leitao.pdf
//!
//! Main differences are:
//!
//! 1. There is no `max hop`.  Messages are signed with a local wallclock.  If they are outside of
//!    the local nodes wallclock window they are dropped silently.
//! 2. The prune set is stored in a Bloom filter.
//! 
//! Plumtree 是一种优化的 分层广播协议，它帮助 Solana 网络高效地传播消息和数据（如交易、区块、验证信息等），
//! 避免了传统广播协议中的不必要的冗余传输，确保数据传播更加高效和一致。
//! 节点选择： 每个节点不仅仅是盲目地将消息广播给所有节点，而是根据一定的策略选择一部分节点进行传播。
//! 减少冗余： 通过 Plumtree，网络中的每个节点不会向自己已经接收到的节点重新发送相同的数据，从而减少了冗余传播。
//! 分层传播： Plumtree 的一个关键特性是它会根据节点的层级关系来选择传播路径。某个节点（比如 A）会选择特定的其他节点（比如 B、C）来进行消息传播，而这些节点再将信息传播给它们的子节点，形成树状传播结构。
//! 每个节点的选择： 节点根据一定的规则（比如距离、节点的重要性、节点的“信誉”）来选择哪些节点接收消息。这样可以确保最重要的节点接收到最及时的数据，减少不必要的消息传递。

use {
    crate::{
        cluster_info::CRDS_UNIQUE_PUBKEY_CAPACITY,
        crds::{Crds, CrdsError, Cursor, GossipRoute},
        crds_gossip,
        crds_value::CrdsValue,
        protocol::{Ping, PingCache},
        push_active_set::PushActiveSet,
        received_cache::ReceivedCache,
    },
    itertools::Itertools,
    solana_sdk::{
        pubkey::Pubkey,
        signature::{Keypair, Signer},
        timing::timestamp,
    },
    solana_streamer::socket::SocketAddrSpace,
    std::{
        collections::{HashMap, HashSet},
        iter::repeat,
        net::SocketAddr,
        ops::{DerefMut, RangeBounds},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex, RwLock,
        },
    },
};

/// 八卦一次发送给多少节点
const CRDS_GOSSIP_PUSH_FANOUT: usize = 9;
// With a fanout of 9, a 2000 node cluster should only take ~3.5 hops to converge.
// However since pushes are stake weighed, some trailing nodes
// might need more time to receive values. 30 seconds should be plenty.
/// 八卦发送超时时间，30s
pub const CRDS_GOSSIP_PUSH_MSG_TIMEOUT_MS: u64 = 30000;
/// 八卦修剪信息超时时间
const CRDS_GOSSIP_PRUNE_MSG_TIMEOUT_MS: u64 = 500;
/// 八卦修剪质押门槛pct
const CRDS_GOSSIP_PRUNE_STAKE_THRESHOLD_PCT: f64 = 0.15;
/// 八卦修剪最小入站节点数
const CRDS_GOSSIP_PRUNE_MIN_INGRESS_NODES: usize = 2;
/// 八卦最大发送集大小
const CRDS_GOSSIP_PUSH_ACTIVE_SET_SIZE: usize = CRDS_GOSSIP_PUSH_FANOUT + 3;

/// 八卦发送器
pub struct CrdsGossipPush {
    /// Active set of validators for push
    /// 推送目标节点集名单，已经按质押量排序，维护好了，每次有需要推送的crds值，都会从这里取节点名单进行推送
    active_set: RwLock<PushActiveSet>,
    /// Cursor into the crds table for values to push.
    /// 指向要发送的 crds 项的游标
    crds_cursor: Mutex<Cursor>,
    /// Cache that tracks which validators a message was received from
    /// This cache represents a lagging view of which validators
    /// currently have this node in their `active_set`
    /// 跟踪从哪个验证器接收到消息的缓存。
    /// 此缓存表示当前哪些验证器在其active_set中具有此节点的滞后视图
    /// 消息来源节点，和他们发送的消息的重复度得分
    received_cache: Mutex<ReceivedCache>,
    /// 每次发送到节点的个数，可能会按这个进行切分批次
    push_fanout: usize,
    /// 消息超时
    pub(crate) msg_timeout: u64,
    /// 清理超时
    pub prune_timeout: u64,
    /// 发送过的总数
    pub num_total: AtomicUsize,
    /// 旧数
    pub num_old: AtomicUsize,
    /// 发送数
    pub num_pushes: AtomicUsize,
}

impl Default for CrdsGossipPush {
    fn default() -> Self {
        Self {
            active_set: RwLock::default(),
            crds_cursor: Mutex::default(),
            received_cache: Mutex::new(ReceivedCache::new(2 * CRDS_UNIQUE_PUBKEY_CAPACITY)),
            push_fanout: CRDS_GOSSIP_PUSH_FANOUT,
            msg_timeout: CRDS_GOSSIP_PUSH_MSG_TIMEOUT_MS,
            prune_timeout: CRDS_GOSSIP_PRUNE_MSG_TIMEOUT_MS,
            num_total: AtomicUsize::default(),
            num_old: AtomicUsize::default(),
            num_pushes: AtomicUsize::default(),
        }
    }
}
impl CrdsGossipPush {
    /// 还有多少没有发送
    pub fn num_pending(&self, crds: &RwLock<Crds>) -> usize {
        /// 获取当前游标
        let mut cursor: Cursor = *self.crds_cursor.lock().unwrap();
        /// 获取游标后的项个数
        crds.read().unwrap().get_entries(&mut cursor).count()
    }

    /// 清理接收缓存
    /// 传入一些来源节点公钥
    pub(crate) fn prune_received_cache<I>(
        &self,
        self_pubkey: &Pubkey,
        origins: I, // Unique pubkeys of crds values' owners.
        stakes: &HashMap<Pubkey, u64>,
    ) -> HashMap</*gossip peer:*/ Pubkey, /*origins:*/ Vec<Pubkey>>
    where
        I: IntoIterator<Item = Pubkey>,
    {
        let mut received_cache = self.received_cache.lock().unwrap();
        origins
            .into_iter()
            .flat_map(|origin| {
                received_cache
                    /// 清理对应公钥的
                    .prune(
                        self_pubkey,
                        origin,
                        CRDS_GOSSIP_PRUNE_STAKE_THRESHOLD_PCT,
                        CRDS_GOSSIP_PRUNE_MIN_INGRESS_NODES,
                        stakes,
                    )
                    .zip(repeat(origin))
            })
            .into_group_map()
    }

    /// 现实时间窗口，从比现在少个超时时间，到比现在多个超时时间
    fn wallclock_window(&self, now: u64) -> impl RangeBounds<u64> {
        now.saturating_sub(self.msg_timeout)..=now.saturating_add(self.msg_timeout)
    }

    /// Process a push message to the network.
    ///
    /// Returns origins' pubkeys of upserted values.
    /// 处理接收到的推送消息
    pub(crate) fn process_push_message(
        &self,
        crds: &RwLock<Crds>,
        messages: Vec<(/*from:*/ Pubkey, Vec<CrdsValue>)>,
        now: u64,
    ) -> HashSet<Pubkey> {
        /// 接收缓存
        let mut received_cache = self.received_cache.lock().unwrap();
        /// crds
        let mut crds = crds.write().unwrap();
        /// 现实时间窗口
        let wallclock_window = self.wallclock_window(now);
        /// 来源节点集
        let mut origins = HashSet::new();
        /// 遍历消息
        for (from, values) in messages {
            /// 总数计数增加
            self.num_total.fetch_add(values.len(), Ordering::Relaxed);
            /// 遍历值
            for value in values {
                /// 值的现实时间不在时间窗口里则跳过
                if !wallclock_window.contains(&value.wallclock()) {
                    continue;
                }
                /// 来源
                let origin = value.pubkey();
                match crds.insert(value, now, GossipRoute::PushMessage(&from)) {
                    Ok(()) => {
                        received_cache.record(origin, from, /*num_dups:*/ 0);
                        origins.insert(origin);
                    }
                    Err(CrdsError::DuplicatePush(num_dups)) => {
                        received_cache.record(origin, from, usize::from(num_dups));
                        self.num_old.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(CrdsError::InsertFailed | CrdsError::UnknownStakes) => {
                        received_cache.record(origin, from, /*num_dups:*/ usize::MAX);
                        self.num_old.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        origins
    }

    /// New push message to broadcast to peers.
    ///
    /// Returns a list of Pubkeys for the selected peers and a list of values to send to all the
    /// peers.
    /// The list of push messages is created such that all the randomly selected peers have not
    /// pruned the source addresses.
    /// 构造即将推送的消息列表
    /// 传入当前节点公钥，crds表，当前时间，质押量
    /// 返回推送消息列表，推送消息个数，推送值个数
    pub(crate) fn new_push_messages(
        &self,
        pubkey: &Pubkey, // This node.
        crds: &RwLock<Crds>,
        now: u64,
        stakes: &HashMap<Pubkey, u64>,
    ) -> (
        HashMap<Pubkey, Vec<CrdsValue>>,
        usize, // number of values
        usize, // number of push messages
    ) {
        // 推送消息最大个数
        const MAX_NUM_PUSHES: usize = 1 << 12;
        // 推送目标节点集名单，已经按质押量排序，维护好了，每次有需要推送的crds值，都会从这里取节点名单进行推送
        let active_set = self.active_set.read().unwrap();
        let mut num_pushes = 0;
        let mut num_values = 0;
        let mut push_messages: HashMap<Pubkey, Vec<CrdsValue>> = HashMap::new();
        let wallclock_window = self.wallclock_window(now);
        let mut crds_cursor = self.crds_cursor.lock().unwrap();
        // crds should be locked last after self.{active_set,crds_cursor}.
        let crds = crds.read().unwrap();
        // 获取所有剩余未发送的crds值
        let entries = crds
            .get_entries(crds_cursor.deref_mut())
            .map(|entry| &entry.value)
            .filter(|value| wallclock_window.contains(&value.wallclock()));
        // 遍历所有剩余未发送的crds值
        // 双层循环，外层循环遍历所有剩余未发送的crds值，内层循环遍历推送目标节点集名单
        // 把每个值发给每个名单上的节点
        'outer: for value in entries {
            // 推送值个数增加
            num_values += 1;
            // 获取推送值的来源节点
            let origin = value.pubkey();
            // 获取推送目标节点集名单
            let nodes = active_set.get_nodes(
                pubkey,
                &origin,
                |node| value.should_force_push(node),
                stakes,
            );
            for node in nodes.take(self.push_fanout) {
                // 推送消息列表增加推送值
                push_messages.entry(*node).or_default().push(value.clone());
                // 推送消息个数增加
                num_pushes += 1;
                // 推送消息个数超过最大个数则跳出
                if num_pushes >= MAX_NUM_PUSHES {
                    break 'outer;
                }
            }
        }
        // 释放crds
        drop(crds);
        // 释放crds游标
        drop(crds_cursor);
        // 释放推送目标节点集名单
        drop(active_set);
        // 推送消息个数增加
        self.num_pushes.fetch_add(num_pushes, Ordering::Relaxed);
        // 返回推送消息列表，推送消息个数，推送值个数
        (push_messages, num_values, num_pushes)
    }

    /// Add the `from` to the peer's filter of nodes.
    /// 将某些节点加入不推送名单
    pub(crate) fn process_prune_msg(
        &self,
        self_pubkey: &Pubkey,
        peer: &Pubkey,
        origins: &[Pubkey],
        stakes: &HashMap<Pubkey, u64>,
    ) {
        let active_set = self.active_set.read().unwrap();
        active_set.prune(self_pubkey, peer, origins, stakes);
    }

    /// Refresh the push active set.
    /// 刷新推送目标节点集名单，重新排序和洗牌
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn refresh_push_active_set(
        &self,
        crds: &RwLock<Crds>,
        stakes: &HashMap<Pubkey, u64>,
        gossip_validators: Option<&HashSet<Pubkey>>,
        self_keypair: &Keypair,
        self_shred_version: u16,
        ping_cache: &Mutex<PingCache>,
        pings: &mut Vec<(SocketAddr, Ping)>,
        socket_addr_space: &SocketAddrSpace,
    ) {
        // 随机数生成器
        let mut rng = rand::thread_rng();
        // Active and valid gossip nodes with matching shred-version.
        // 获取所有活跃和有效的八卦节点
        let nodes = crds_gossip::get_gossip_nodes(
            &mut rng,
            timestamp(), // now
            &self_keypair.pubkey(),
            // Only push to nodes with the same shred version.
            |shred_version| shred_version == self_shred_version,
            crds,
            gossip_validators,
            stakes,
            socket_addr_space,
        );
        // Check for nodes which have responded to ping messages.
        // 检查哪些节点已经响应了ping消息
        let nodes = crds_gossip::maybe_ping_gossip_addresses(
            &mut rng,
            nodes,
            self_keypair,
            ping_cache,
            pings,
        );
        // 去重
        let nodes = crds_gossip::dedup_gossip_addresses(nodes, stakes)
            .into_values()
            .map(|(_stake, node)| *node.pubkey())
            .collect::<Vec<_>>();
        // 如果节点列表为空，则返回
        if nodes.is_empty() {
            return;
        }
        // 获取crds表中节点个数和质押量个数的最大值
        let cluster_size = crds.read().unwrap().num_pubkeys().max(stakes.len());
        // 写锁
        let mut active_set = self.active_set.write().unwrap();
        // 刷新推送目标节点集名单
        active_set.rotate(
            &mut rng,
            CRDS_GOSSIP_PUSH_ACTIVE_SET_SIZE,
            cluster_size,
            &nodes,
            stakes,
        )
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{contact_info::ContactInfo, crds_data::CrdsData},
        std::time::{Duration, Instant},
    };

    fn new_ping_cache() -> PingCache {
        PingCache::new(
            &mut rand::thread_rng(),
            Instant::now(),
            Duration::from_secs(20 * 60),      // ttl
            Duration::from_secs(20 * 60) / 64, // rate_limit_delay
            128,                               // capacity
        )
    }

    #[test]
    fn test_process_push_one() {
        let crds = RwLock::<Crds>::default();
        let push = CrdsGossipPush::default();
        let value = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::new_localhost(
            &solana_sdk::pubkey::new_rand(),
            0,
        )));
        let label = value.label();
        // push a new message
        assert_eq!(
            push.process_push_message(&crds, vec![(Pubkey::default(), vec![value.clone()])], 0),
            [label.pubkey()].into_iter().collect(),
        );
        assert_eq!(crds.read().unwrap().get::<&CrdsValue>(&label), Some(&value));

        // push it again
        assert!(push
            .process_push_message(&crds, vec![(Pubkey::default(), vec![value])], 0)
            .is_empty());
    }
    #[test]
    fn test_process_push_old_version() {
        let crds = RwLock::<Crds>::default();
        let push = CrdsGossipPush::default();
        let mut ci = ContactInfo::new_localhost(&solana_sdk::pubkey::new_rand(), 0);
        ci.set_wallclock(1);
        let value = CrdsValue::new_unsigned(CrdsData::ContactInfo(ci.clone()));

        // push a new message
        assert_eq!(
            push.process_push_message(&crds, vec![(Pubkey::default(), vec![value])], 0),
            [*ci.pubkey()].into_iter().collect()
        );

        // push an old version
        ci.set_wallclock(0);
        let value = CrdsValue::new_unsigned(CrdsData::ContactInfo(ci));
        assert!(push
            .process_push_message(&crds, vec![(Pubkey::default(), vec![value])], 0)
            .is_empty());
    }
    #[test]
    fn test_process_push_timeout() {
        let crds = RwLock::<Crds>::default();
        let push = CrdsGossipPush::default();
        let timeout = push.msg_timeout;
        let mut ci = ContactInfo::new_localhost(&solana_sdk::pubkey::new_rand(), 0);

        // push a version to far in the future
        ci.set_wallclock(timeout + 1);
        let value = CrdsValue::new_unsigned(CrdsData::ContactInfo(ci.clone()));
        assert!(push
            .process_push_message(&crds, vec![(Pubkey::default(), vec![value])], 0)
            .is_empty());

        // push a version to far in the past
        ci.set_wallclock(0);
        let value = CrdsValue::new_unsigned(CrdsData::ContactInfo(ci));
        assert!(push
            .process_push_message(&crds, vec![(Pubkey::default(), vec![value])], timeout + 1)
            .is_empty());
    }
    #[test]
    fn test_process_push_update() {
        let crds = RwLock::<Crds>::default();
        let push = CrdsGossipPush::default();
        let mut ci = ContactInfo::new_localhost(&solana_sdk::pubkey::new_rand(), 0);
        let origin = *ci.pubkey();
        ci.set_wallclock(0);
        let value_old = CrdsValue::new_unsigned(CrdsData::ContactInfo(ci.clone()));

        // push a new message
        assert_eq!(
            push.process_push_message(&crds, vec![(Pubkey::default(), vec![value_old])], 0),
            [origin].into_iter().collect()
        );

        // push an old version
        ci.set_wallclock(1);
        let value = CrdsValue::new_unsigned(CrdsData::ContactInfo(ci));
        assert_eq!(
            push.process_push_message(&crds, vec![(Pubkey::default(), vec![value])], 0),
            [origin].into_iter().collect()
        );
    }

    #[test]
    fn test_new_push_messages() {
        let now = timestamp();
        let mut crds = Crds::default();
        let push = CrdsGossipPush::default();
        let mut ping_cache = new_ping_cache();
        let peer = ContactInfo::new_localhost(&solana_sdk::pubkey::new_rand(), 0);
        ping_cache.mock_pong(*peer.pubkey(), peer.gossip().unwrap(), Instant::now());
        let peer = CrdsValue::new_unsigned(CrdsData::ContactInfo(peer));
        assert_eq!(
            crds.insert(peer.clone(), now, GossipRoute::LocalMessage),
            Ok(())
        );
        let crds = RwLock::new(crds);
        let ping_cache = Mutex::new(ping_cache);
        push.refresh_push_active_set(
            &crds,
            &HashMap::new(), // stakes
            None,            // gossip_validtors
            &Keypair::new(),
            0, // self_shred_version
            &ping_cache,
            &mut Vec::new(), // pings
            &SocketAddrSpace::Unspecified,
        );

        let new_msg = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::new_localhost(
            &solana_sdk::pubkey::new_rand(),
            0,
        )));
        let mut expected = HashMap::new();
        expected.insert(peer.label().pubkey(), vec![new_msg.clone()]);
        let origin = new_msg.pubkey();
        assert_eq!(
            push.process_push_message(&crds, vec![(Pubkey::default(), vec![new_msg])], 0),
            [origin].into_iter().collect()
        );
        assert_eq!(
            push.new_push_messages(
                &Pubkey::default(),
                &crds,
                0,
                &HashMap::<Pubkey, u64>::default(), // stakes
            )
            .0,
            expected
        );
    }
    #[test]
    fn test_personalized_push_messages() {
        let now = timestamp();
        let mut rng = rand::thread_rng();
        let mut crds = Crds::default();
        let push = CrdsGossipPush::default();
        let mut ping_cache = new_ping_cache();
        let peers: Vec<_> = vec![0, 0, now]
            .into_iter()
            .map(|wallclock| {
                let mut peer = ContactInfo::new_rand(&mut rng, /*pubkey=*/ None);
                peer.set_wallclock(wallclock);
                ping_cache.mock_pong(*peer.pubkey(), peer.gossip().unwrap(), Instant::now());
                CrdsValue::new_unsigned(CrdsData::ContactInfo(peer))
            })
            .collect();
        let origin: Vec<_> = peers.iter().map(|node| node.pubkey()).collect();
        assert_eq!(
            crds.insert(peers[0].clone(), now, GossipRoute::LocalMessage),
            Ok(())
        );
        assert_eq!(
            crds.insert(peers[1].clone(), now, GossipRoute::LocalMessage),
            Ok(())
        );
        let crds = RwLock::new(crds);
        assert_eq!(
            push.process_push_message(
                &crds,
                vec![(Pubkey::default(), vec![peers[2].clone()])],
                now
            ),
            [origin[2]].into_iter().collect()
        );
        let ping_cache = Mutex::new(ping_cache);
        push.refresh_push_active_set(
            &crds,
            &HashMap::new(), // stakes
            None,            // gossip_validators
            &Keypair::new(),
            0, // self_shred_version
            &ping_cache,
            &mut Vec::new(),
            &SocketAddrSpace::Unspecified,
        );

        // push 3's contact info to 1 and 2 and 3
        let expected: HashMap<_, _> = vec![
            (peers[0].pubkey(), vec![peers[2].clone()]),
            (peers[1].pubkey(), vec![peers[2].clone()]),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            push.new_push_messages(
                &Pubkey::default(),
                &crds,
                now,
                &HashMap::<Pubkey, u64>::default(), // stakes
            )
            .0,
            expected
        );
    }
    #[test]
    fn test_process_prune() {
        let mut crds = Crds::default();
        let self_id = solana_sdk::pubkey::new_rand();
        let push = CrdsGossipPush::default();
        let peer = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::new_localhost(
            &solana_sdk::pubkey::new_rand(),
            0,
        )));
        assert_eq!(
            crds.insert(peer.clone(), 0, GossipRoute::LocalMessage),
            Ok(())
        );
        let crds = RwLock::new(crds);
        let ping_cache = Mutex::new(new_ping_cache());
        push.refresh_push_active_set(
            &crds,
            &HashMap::new(), // stakes
            None,            // gossip_validators
            &Keypair::new(),
            0, // self_shred_version
            &ping_cache,
            &mut Vec::new(), // pings
            &SocketAddrSpace::Unspecified,
        );

        let new_msg = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::new_localhost(
            &solana_sdk::pubkey::new_rand(),
            0,
        )));
        let expected = HashMap::new();
        let origin = new_msg.pubkey();
        assert_eq!(
            push.process_push_message(&crds, vec![(Pubkey::default(), vec![new_msg.clone()])], 0),
            [origin].into_iter().collect()
        );
        push.process_prune_msg(
            &self_id,
            &peer.label().pubkey(),
            &[new_msg.label().pubkey()],
            &HashMap::<Pubkey, u64>::default(), // stakes
        );
        assert_eq!(
            push.new_push_messages(
                &self_id,
                &crds,
                0,
                &HashMap::<Pubkey, u64>::default(), // stakes
            )
            .0,
            expected
        );
    }
    #[test]
    fn test_purge_old_pending_push_messages() {
        let mut crds = Crds::default();
        let push = CrdsGossipPush::default();
        let peer = CrdsValue::new_unsigned(CrdsData::ContactInfo(ContactInfo::new_localhost(
            &solana_sdk::pubkey::new_rand(),
            0,
        )));
        assert_eq!(crds.insert(peer, 0, GossipRoute::LocalMessage), Ok(()));
        let crds = RwLock::new(crds);
        let ping_cache = Mutex::new(new_ping_cache());
        push.refresh_push_active_set(
            &crds,
            &HashMap::new(), // stakes
            None,            // gossip_validators
            &Keypair::new(),
            0, // self_shred_version
            &ping_cache,
            &mut Vec::new(), // pings
            &SocketAddrSpace::Unspecified,
        );

        let mut ci = ContactInfo::new_localhost(&solana_sdk::pubkey::new_rand(), 0);
        ci.set_wallclock(1);
        let new_msg = CrdsValue::new_unsigned(CrdsData::ContactInfo(ci));
        let expected = HashMap::new();
        let origin = new_msg.pubkey();
        assert_eq!(
            push.process_push_message(&crds, vec![(Pubkey::default(), vec![new_msg])], 1),
            [origin].into_iter().collect()
        );
        assert_eq!(
            push.new_push_messages(
                &Pubkey::default(),
                &crds,
                0,
                &HashMap::<Pubkey, u64>::default(), // stakes
            )
            .0,
            expected
        );
    }

    #[test]
    fn test_purge_old_received_cache() {
        let crds = RwLock::<Crds>::default();
        let push = CrdsGossipPush::default();
        let mut ci = ContactInfo::new_localhost(&solana_sdk::pubkey::new_rand(), 0);
        ci.set_wallclock(0);
        let value = CrdsValue::new_unsigned(CrdsData::ContactInfo(ci));
        let label = value.label();
        // push a new message
        assert_eq!(
            push.process_push_message(&crds, vec![(Pubkey::default(), vec![value.clone()])], 0),
            [label.pubkey()].into_iter().collect()
        );
        assert_eq!(
            crds.write().unwrap().get::<&CrdsValue>(&label),
            Some(&value)
        );

        // push it again
        assert!(push
            .process_push_message(&crds, vec![(Pubkey::default(), vec![value.clone()])], 0)
            .is_empty());

        // push it again
        assert!(push
            .process_push_message(&crds, vec![(Pubkey::default(), vec![value])], 0)
            .is_empty());
    }
}
