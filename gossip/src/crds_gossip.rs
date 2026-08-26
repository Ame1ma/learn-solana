//! Crds Gossip.
//!
//! This module ties together Crds and the push and pull gossip overlays.  The interface is
//! designed to run with a simulator or over a UDP network connection with messages up to a
//! packet::PACKET_DATA_SIZE size.

use {
    crate::{
        cluster_info_metrics::GossipStats,
        contact_info::ContactInfo,
        crds::{Crds, GossipRoute},
        crds_data::CrdsData,
        crds_gossip_error::CrdsGossipError,
        crds_gossip_pull::{CrdsFilter, CrdsGossipPull, CrdsTimeouts, ProcessPullStats},
        crds_gossip_push::CrdsGossipPush,
        crds_value::CrdsValue,
        duplicate_shred::{self, DuplicateShredIndex, MAX_DUPLICATE_SHREDS},
        protocol::{Ping, PingCache},
    },
    itertools::Itertools,
    rand::{CryptoRng, Rng},
    rayon::ThreadPool,
    solana_ledger::shred::Shred,
    solana_sdk::{
        clock::Slot,
        hash::Hash,
        pubkey::Pubkey,
        signature::{Keypair, Signer},
        timing::timestamp,
    },
    solana_streamer::socket::SocketAddrSpace,
    std::{
        collections::{HashMap, HashSet},
        net::SocketAddr,
        sync::{Mutex, RwLock},
        time::{Duration, Instant},
    },
};

/// 在 **Solana** 网络中，`CRDS Gossip`（**Crds Gossip**）是指一种用于在网络中传播和共享**节点信息**、**状态信息**的机制。它是 **CRDS**（**Cluster Radio Gossip Database System**）的一部分，旨在帮助验证者节点通过点对点的方式传递关于网络状态的关键数据。
/// ### 1. **CRDS（Cluster Radio Gossip Database System）是什么？**
/// CRDS 是 Solana 网络中用于分布式系统状态传播和信息共享的基础系统之一。它通过**Gossip协议**（即点对点信息广播）来实现网络中节点之间的信息同步。Gossip 是一种高效的传播信息的方式，节点通过广播自己的信息，使得其他节点能逐渐知道网络中其他节点的状态。
/// 在 Solana 中，CRDS 系统包含了不同类型的数据，这些数据会被不同的节点通过 gossip 广播。比如，节点的**状态**、**验证者身份**、**区块信息**等。它是 Solana 网络分布式状态管理的重要组成部分。
/// ### 2. **Gossip 在 CRDS 中的作用**
/// Gossip 是一种信息传递协议，它允许网络中的节点以高效的方式传播消息。在 Solana 中，CRDS Gossip 的作用是将网络中的信息（如验证者信息、交易状态等）通过 Gossip 协议广播到其他节点。每个节点不仅仅是接收这些消息，还会根据这些消息更新自己的网络状态，确保每个节点的网络视图保持一致。
/// 具体来说，CRDS Gossip 主要负责以下几个方面的信息传播：
/// - **节点状态信息**：包括节点是否在线、节点的性能、节点是否处于领导状态等。
/// - **区块和投票信息**：验证者投票的结果、区块是否被确认、验证者的投票情况等。
/// - **网络健康状态**：比如当前网络是否有分叉，或者节点是否存在延迟等。
/// ### 3. **CRDS Gossip 的工作流程**
/// CRDS Gossip 通过点对点（peer-to-peer）方式将信息在网络中传播。具体来说，节点会：
/// 1. **发送 Gossip 消息**：每个 Solana 节点会周期性地将自己的信息（例如其投票、状态、交易等）通过 Gossip 协议广播给其他节点。
/// 2. **接收 Gossip 消息**：其他节点收到这些信息后，会对其状态进行更新。这些信息可能会被用来更新网络的当前视图，帮助节点做出下一步的决策，比如投票给某个区块。
/// 3. **同步数据**：节点会不断通过 Gossip 接收和广播信息，从而保持网络中数据的一致性。例如，当一个节点接收到某个区块的消息时，它可能会更新自己的状态，并将这个区块传播给其他节点。
/// ### 4. **CRDS Gossip 的优点**
/// - **去中心化**：每个节点都是信息传播的主体，避免了单点故障的风险。
/// - **高效**：Gossip 协议是一种非常高效的信息传播方式，通过仅传递差异化的信息（即只有更新的内容），减少了网络带宽的消耗。
/// - **容错性强**：如果某些节点在网络中失联，其他节点仍然可以通过多路径传播来获取信息，从而增强了网络的容错能力。
/// ### 5. **CRDS Gossip 的关键组件**
/// Solana 的 CRDS Gossip 系统涉及以下几个重要的概念：
/// - **CRDS Table**：这是 Solana 中用来存储信息的主要数据结构。每个节点都有自己的 CRDS 表，存储着所有需要共享的状态和信息（如节点的状态、投票信息等）。
/// - **Gossip 消息**：Gossip 消息是通过网络传递的信息，节点通过这些消息更新自己的 CRDS 表。
/// - **节点传播策略**：Solana 会根据节点的优先级、网络状况等决定消息的传播策略。例如，某些节点会优先传播给权重更大的节点，或者更快地传播某些关键的网络信息。
/// ### 6. **CRDS Gossip 与其他通信机制的关系**
/// CRDS Gossip 是 Solana 网络中的核心信息传播机制之一，和其他通信机制（如 `RPC`、`Tpu` 等）互补：
/// - **RPC**（远程过程调用）主要用于客户端与服务器之间的交互，提供请求和响应的机制，通常用于查询网络状态。
/// - **TPU**（Transaction Processing Unit）用于处理和广播交易，它关注交易的高效传播。
/// - **CRDS Gossip** 则更侧重于节点间的网络状态信息的传播，比如区块、投票、网络状态等。这些信息通过 Gossip 协议传播到整个网络，确保所有节点对网络的状态保持一致。
/// ### 总结
/// `CRDS Gossip` 是 Solana 网络中用来在节点之间传播和共享关键状态信息的机制。通过 Gossip 协议，Solana 实现了高效、去中心化的信息同步，确保网络中各个节点能够快速且一致地了解网络状态。它是 Solana 构建高吞吐量、高可扩展性区块链的关键组成部分之一。
#[derive(Default)]
pub struct CrdsGossip {
    /// 八卦表
    pub crds: RwLock<Crds>,
    /// 八卦发送
    pub push: CrdsGossipPush,
    /// 八卦接收
    pub pull: CrdsGossipPull,
}

impl CrdsGossip {
    /// Process a push message to the network.
    ///
    /// Returns unique origins' pubkeys of upserted values.
    /// 处理接收到的推送消息
    pub fn process_push_message(
        &self,
        messages: Vec<(/*from:*/ Pubkey, Vec<CrdsValue>)>,
        now: u64,
    ) -> HashSet<Pubkey> {
        self.push.process_push_message(&self.crds, messages, now)
    }

    /// Remove redundant paths in the network.
    /// 清理接收缓存
    pub fn prune_received_cache<I>(
        &self,
        self_pubkey: &Pubkey,
        origins: I, // Unique pubkeys of crds values' owners.
        stakes: &HashMap<Pubkey, u64>,
    ) -> HashMap</*gossip peer:*/ Pubkey, /*origins:*/ Vec<Pubkey>>
    where
        I: IntoIterator<Item = Pubkey>,
    {
        self.push.prune_received_cache(self_pubkey, origins, stakes)
    }

    /// 生成推送消息
    pub fn new_push_messages(
        &self,
        pubkey: &Pubkey, // This node.
        now: u64,
        stakes: &HashMap<Pubkey, u64>,
    ) -> (
        HashMap<Pubkey, Vec<CrdsValue>>,
        usize, // number of values
        usize, // number of push messages
    ) {
        self.push.new_push_messages(pubkey, &self.crds, now, stakes)
    }

    /// 处理重复消息分片
    pub(crate) fn push_duplicate_shred<F>(
        &self,
        keypair: &Keypair,
        shred: &Shred,
        other_payload: &[u8],
        leader_schedule: Option<F>,
        // Maximum serialized size of each DuplicateShred chunk payload.
        max_payload_size: usize,
        shred_version: u16,
    ) -> Result<(), duplicate_shred::Error>
    where
        F: FnOnce(Slot) -> Option<Pubkey>,
    {
        // 获取节点公钥
        let pubkey = keypair.pubkey();
        // Skip if there are already records of duplicate shreds for this slot.
        // 如果已经有重复的消息分片，则跳过
        let shred_slot = shred.slot();
        let mut crds = self.crds.write().unwrap();
        if crds
            .get_records(&pubkey)
            .any(|value| match value.value.data() {
                CrdsData::DuplicateShred(_, value) => value.slot == shred_slot,
                _ => false,
            })
        {
            return Ok(());
        }
        let chunks = duplicate_shred::from_shred(
            shred.clone(),
            pubkey,
            Vec::from(other_payload),
            leader_schedule,
            timestamp(),
            max_payload_size,
            shred_version,
        )?;
        // Find the index of oldest duplicate shred.
        let mut num_dup_shreds = 0;
        let offset = crds
            .get_records(&pubkey)
            .filter_map(|value| match value.value.data() {
                CrdsData::DuplicateShred(ix, value) => {
                    num_dup_shreds += 1;
                    Some((value.wallclock, *ix))
                }
                _ => None,
            })
            .min() // Override the oldest records.
            .map(|(_ /*wallclock*/, ix)| ix)
            .unwrap_or(0);
        let offset = if num_dup_shreds < MAX_DUPLICATE_SHREDS {
            num_dup_shreds
        } else {
            offset
        };
        let entries = chunks.enumerate().map(|(k, chunk)| {
            let index = (offset + k as DuplicateShredIndex) % MAX_DUPLICATE_SHREDS;
            let data = CrdsData::DuplicateShred(index, chunk);
            CrdsValue::new(data, keypair)
        });
        let now = timestamp();
        for entry in entries {
            if let Err(err) = crds.insert(entry, now, GossipRoute::LocalMessage) {
                error!("push_duplicate_shred failed: {:?}", err);
            }
        }
        Ok(())
    }

    /// Add the `from` to the peer's filter of nodes.
    pub fn process_prune_msg(
        &self,
        self_pubkey: &Pubkey,
        peer: &Pubkey,
        destination: &Pubkey,
        origin: &[Pubkey],
        wallclock: u64,
        now: u64,
        stakes: &HashMap<Pubkey, u64>,
    ) -> Result<(), CrdsGossipError> {
        if now > wallclock.saturating_add(self.push.prune_timeout) {
            Err(CrdsGossipError::PruneMessageTimeout)
        } else if self_pubkey == destination {
            self.push
                .process_prune_msg(self_pubkey, peer, origin, stakes);
            Ok(())
        } else {
            Err(CrdsGossipError::BadPruneDestination)
        }
    }

    /// Refresh the push active set.
    pub fn refresh_push_active_set(
        &self,
        self_keypair: &Keypair,
        self_shred_version: u16,
        stakes: &HashMap<Pubkey, u64>,
        gossip_validators: Option<&HashSet<Pubkey>>,
        ping_cache: &Mutex<PingCache>,
        pings: &mut Vec<(SocketAddr, Ping)>,
        socket_addr_space: &SocketAddrSpace,
    ) {
        self.push.refresh_push_active_set(
            &self.crds,
            stakes,
            gossip_validators,
            self_keypair,
            self_shred_version,
            ping_cache,
            pings,
            socket_addr_space,
        )
    }

    /// Generate a random request.
    #[allow(clippy::too_many_arguments)]
    pub fn new_pull_request(
        &self,
        thread_pool: &ThreadPool,
        self_keypair: &Keypair,
        self_shred_version: u16,
        now: u64,
        gossip_validators: Option<&HashSet<Pubkey>>,
        stakes: &HashMap<Pubkey, u64>,
        bloom_size: usize,
        ping_cache: &Mutex<PingCache>,
        pings: &mut Vec<(SocketAddr, Ping)>,
        socket_addr_space: &SocketAddrSpace,
    ) -> Result<Vec<(ContactInfo, Vec<CrdsFilter>)>, CrdsGossipError> {
        self.pull.new_pull_request(
            thread_pool,
            &self.crds,
            self_keypair,
            self_shred_version,
            now,
            gossip_validators,
            stakes,
            bloom_size,
            ping_cache,
            pings,
            socket_addr_space,
        )
    }

    pub fn generate_pull_responses(
        &self,
        thread_pool: &ThreadPool,
        filters: &[(CrdsValue, CrdsFilter)],
        output_size_limit: usize, // Limit number of crds values returned.
        now: u64,
        stats: &GossipStats,
    ) -> Vec<Vec<CrdsValue>> {
        CrdsGossipPull::generate_pull_responses(
            thread_pool,
            &self.crds,
            filters,
            output_size_limit,
            now,
            stats,
        )
    }

    pub fn filter_pull_responses(
        &self,
        timeouts: &CrdsTimeouts,
        response: Vec<CrdsValue>,
        now: u64,
        process_pull_stats: &mut ProcessPullStats,
    ) -> (
        Vec<CrdsValue>, // valid responses.
        Vec<CrdsValue>, // responses with expired timestamps.
        Vec<Hash>,      // hash of outdated values.
    ) {
        self.pull
            .filter_pull_responses(&self.crds, timeouts, response, now, process_pull_stats)
    }

    /// Process a pull response.
    pub fn process_pull_responses(
        &self,
        responses: Vec<CrdsValue>,
        responses_expired_timeout: Vec<CrdsValue>,
        failed_inserts: Vec<Hash>,
        now: u64,
        process_pull_stats: &mut ProcessPullStats,
    ) {
        self.pull.process_pull_responses(
            &self.crds,
            responses,
            responses_expired_timeout,
            failed_inserts,
            now,
            process_pull_stats,
        );
    }

    pub fn make_timeouts<'a>(
        &self,
        self_pubkey: Pubkey,
        stakes: &'a HashMap<Pubkey, u64>,
        epoch_duration: Duration,
    ) -> CrdsTimeouts<'a> {
        self.pull.make_timeouts(self_pubkey, stakes, epoch_duration)
    }

    pub fn purge(
        &self,
        self_pubkey: &Pubkey,
        thread_pool: &ThreadPool,
        now: u64,
        timeouts: &CrdsTimeouts,
    ) -> usize {
        let mut rv = 0;
        if now > self.pull.crds_timeout {
            debug_assert_eq!(timeouts[self_pubkey], u64::MAX);
            debug_assert_ne!(timeouts[&Pubkey::default()], 0u64);
            rv = CrdsGossipPull::purge_active(thread_pool, &self.crds, now, timeouts);
        }
        self.crds
            .write()
            .unwrap()
            .trim_purged(now.saturating_sub(5 * self.pull.crds_timeout));
        self.pull.purge_failed_inserts(now);
        rv
    }
}

// Returns active and valid cluster nodes to gossip with.
/// 获取活跃和有效的八卦节点
/// 传入随机数生成器，当前时间，当前节点公钥，分片版本验证函数，crds表，八卦节点允许列表，质押量，节点地址空间
/// 返回活跃和有效的八卦节点列表
pub(crate) fn get_gossip_nodes<R: Rng>(
    rng: &mut R,
    now: u64,
    pubkey: &Pubkey, // This node.
    // By default, should only push to or pull from gossip nodes with the same
    // shred-version. Except for spy nodes (shred_version == 0u16) which can
    // pull from any node.
    verify_shred_version: impl Fn(/*shred_version:*/ u16) -> bool,
    crds: &RwLock<Crds>,
    gossip_validators: Option<&HashSet<Pubkey>>,
    stakes: &HashMap<Pubkey, u64>,
    socket_addr_space: &SocketAddrSpace,
) -> Vec<ContactInfo> {
    // Exclude nodes which have not been active for this long.
    // 排除长时间未活跃的节点，超过这个时间就认为节点不活跃
    const ACTIVE_TIMEOUT: Duration = Duration::from_secs(60);
    // 计算当前时间减去活跃超时时间，得到活跃截止时间
    let active_cutoff = now.saturating_sub(ACTIVE_TIMEOUT.as_millis() as u64);
    // 读取crds表
    let crds = crds.read().unwrap();
    // 获取所有节点
    crds.get_nodes()
        .filter_map(|value| {
            // 获取节点信息
            let node = value.value.contact_info().unwrap();
            // Exclude nodes which have not been active recently.
            // 排除长时间未更新信息的节点，超过这个时间就认为节点不活跃
            if value.local_timestamp < active_cutoff {
                // In order to mitigate eclipse attack, for staked nodes
                // continue retrying periodically.
                // 为了防止 eclipse 攻击，对于质押节点，继续定期重试。
                // 获取节点质押量
                let stake = stakes.get(node.pubkey()).copied().unwrap_or_default();
                // 如果质押量为0，或者随机数生成器生成1/16的概率为false，则认为节点不活跃
                if stake == 0u64 || !rng.gen_ratio(1, 16) {
                    return None;
                }
            }
            Some(node)
        })
        // 排除当前节点，并且分片版本匹配，并且节点地址有效，并且节点在八卦节点允许列表中
        .filter(|node| {
            // 排除当前节点
            node.pubkey() != pubkey
                // 分片版本匹配
                && verify_shred_version(node.shred_version())
                // 节点地址有效
                && node
                    .gossip()
                    .map(|addr| socket_addr_space.check(&addr))
                    .unwrap_or_default()
                // 节点在八卦节点允许列表中
                && match gossip_validators {
                    Some(nodes) => nodes.contains(&node.pubkey()),
                    None => true,
                }
        })
        .cloned()
        .collect()
}

// Dedups gossip addresses, keeping only the one with the highest stake.
/// 去重八卦地址，nodes 记录中可能会有重复的节点，这里将每个节点只保留质押量最高的一条记录
pub(crate) fn dedup_gossip_addresses(
    nodes: impl IntoIterator<Item = ContactInfo>,
    stakes: &HashMap<Pubkey, u64>,
) -> HashMap</*gossip:*/ SocketAddr, (/*stake:*/ u64, ContactInfo)> {
    nodes
        .into_iter()
        .filter_map(|node| Some((node.gossip().ok()?, node)))
        // 同一个节点可能会有多个记录，先按节点地址分组
        .into_grouping_map()
        // 再取每个组中质押量最高的
        .aggregate(|acc, _node_gossip, node| {
            // 获取节点质押量
            let stake = stakes.get(node.pubkey()).copied().unwrap_or_default();
            // 如果acc存在，并且acc的质押量大于等于当前节点质押量，则返回acc，否则返回当前节点
            match acc {
                Some((ref s, _)) if s >= &stake => acc,
                Some(_) | None => Some((stake, node)),
            }
        })
}

// Pings gossip addresses if needed.
// Returns nodes which have recently responded to a ping message.
/// 最近响应ping消息的节点
#[must_use]
pub(crate) fn maybe_ping_gossip_addresses<R: Rng + CryptoRng>(
    rng: &mut R,
    nodes: impl IntoIterator<Item = ContactInfo>,
    keypair: &Keypair,
    ping_cache: &Mutex<PingCache>,
    pings: &mut Vec<(SocketAddr, Ping)>,
) -> Vec<ContactInfo> {
    // 获取ping缓存
    let mut ping_cache = ping_cache.lock().unwrap();
    // 获取当前时间
    let now = Instant::now();
    // 遍历节点
    nodes
        .into_iter()
        .filter(|node| {
            // 获取节点地址
            let Ok(node_gossip) = node.gossip() else {
                return false;
            };
            // 检查节点是否收到ping响应
            let (check, ping) = {
                let node = (*node.pubkey(), node_gossip);
                ping_cache.check(rng, keypair, now, node)
            };
            // 如果节点收到ping响应，则添加到ping列表
            if let Some(ping) = ping {
                pings.push((node_gossip, ping));
            }
            check
        })
        .collect()
}

#[cfg(test)]
mod test {
    use {
        super::*,
        solana_sdk::{hash::hash, timing::timestamp},
    };

    #[test]
    fn test_prune_errors() {
        let crds_gossip = CrdsGossip::default();
        let keypair = Keypair::new();
        let id = keypair.pubkey();
        let ci = ContactInfo::new_localhost(&Pubkey::from([1; 32]), 0);
        let prune_pubkey = Pubkey::from([2; 32]);
        crds_gossip
            .crds
            .write()
            .unwrap()
            .insert(
                CrdsValue::new_unsigned(CrdsData::ContactInfo(ci.clone())),
                0,
                GossipRoute::LocalMessage,
            )
            .unwrap();
        let ping_cache = PingCache::new(
            &mut rand::thread_rng(),
            Instant::now(),
            Duration::from_secs(20 * 60),      // ttl
            Duration::from_secs(20 * 60) / 64, // rate_limit_delay
            128,                               // capacity
        );
        let ping_cache = Mutex::new(ping_cache);
        crds_gossip.refresh_push_active_set(
            &keypair,
            0,               // shred version
            &HashMap::new(), // stakes
            None,            // gossip validators
            &ping_cache,
            &mut Vec::new(), // pings
            &SocketAddrSpace::Unspecified,
        );
        let now = timestamp();
        //incorrect dest
        let mut res = crds_gossip.process_prune_msg(
            &id,
            ci.pubkey(),
            &Pubkey::from(hash(&[1; 32]).to_bytes()),
            &[prune_pubkey],
            now,
            now,
            &HashMap::<Pubkey, u64>::default(), // stakes
        );
        assert_eq!(res.err(), Some(CrdsGossipError::BadPruneDestination));
        //correct dest
        res = crds_gossip.process_prune_msg(
            &id,             // self_pubkey
            ci.pubkey(),     // peer
            &id,             // destination
            &[prune_pubkey], // origins
            now,
            now,
            &HashMap::<Pubkey, u64>::default(), // stakes
        );
        res.unwrap();
        //test timeout
        let timeout = now + crds_gossip.push.prune_timeout * 2;
        res = crds_gossip.process_prune_msg(
            &id,             // self_pubkey
            ci.pubkey(),     // peer
            &id,             // destination
            &[prune_pubkey], // origins
            now,
            timeout,
            &HashMap::<Pubkey, u64>::default(), // stakes
        );
        assert_eq!(res.err(), Some(CrdsGossipError::PruneMessageTimeout));
    }
}
