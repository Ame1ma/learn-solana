//! 集群信息指标

use {
    crate::crds_gossip::CrdsGossip,
    itertools::Itertools,
    solana_measure::measure::Measure,
    solana_sdk::{clock::Slot, pubkey::Pubkey},
    std::{
        cmp::Reverse,
        collections::HashMap,
        ops::{Deref, DerefMut},
        sync::atomic::{AtomicU64, Ordering},
        time::Instant,
    },
};

/// 计数器，用来记时间或者次数，用于统计，只记递增的
#[derive(Default)]
pub(crate) struct Counter(AtomicU64);

impl Counter {
    /// 增加耗时记录
    pub(crate) fn add_measure(&self, x: &mut Measure) {
        x.stop();
        self.0.fetch_add(x.as_us(), Ordering::Relaxed);
    }
    /// 增加耗时记录，raw
    pub(crate) fn add_relaxed(&self, x: u64) {
        self.0.fetch_add(x, Ordering::Relaxed);
    }
    /// 清零
    fn clear(&self) -> u64 {
        self.0.swap(0, Ordering::Relaxed)
    }
}

/// 用来包装守卫，在结束时计时
pub(crate) struct TimedGuard<'a, T> {
    guard: T,
    timer: Measure,
    counter: &'a Counter,
}

/// 本身是个守卫，结束时计时
pub(crate) struct ScopedTimer<'a> {
    clock: Instant,
    metric: &'a AtomicU64,
}

impl<'a> From<&'a Counter> for ScopedTimer<'a> {
    // Output should be assigned to a *named* variable, otherwise it is
    // immediately dropped.
    #[must_use]
    fn from(counter: &'a Counter) -> Self {
        Self {
            clock: Instant::now(),
            metric: &counter.0,
        }
    }
}

impl Drop for ScopedTimer<'_> {
    fn drop(&mut self) {
        let micros = self.clock.elapsed().as_micros();
        self.metric.fetch_add(micros as u64, Ordering::Relaxed);
    }
}

impl<'a, T> TimedGuard<'a, T> {
    pub(crate) fn new(guard: T, label: &'static str, counter: &'a Counter) -> Self {
        Self {
            guard,
            timer: Measure::start(label),
            counter,
        }
    }
}

impl<'a, T> Deref for TimedGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<'a, T> DerefMut for TimedGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

impl<'a, T> Drop for TimedGuard<'a, T> {
    fn drop(&mut self) {
        self.counter.add_measure(&mut self.timer);
    }
}

/// 八卦协议的统计信息
#[derive(Default)]
pub struct GossipStats {
    /// 所有tvu节点
    pub(crate) all_tvu_peers: Counter,
    /// 坏的修剪目的地
    pub(crate) bad_prune_destination: Counter,
    /// 入口点2
    pub(crate) entrypoint2: Counter,
    /// 入口点
    pub(crate) entrypoint: Counter,
    /// 纪元时隙填充
    pub(crate) epoch_slots_filled: Counter,
    /// 纪元时隙查找
    pub(crate) epoch_slots_lookup: Counter,
    /// 过滤crds请求
    pub(crate) filter_crds_values_dropped_requests: Counter,
    /// 过滤crds值
    pub(crate) filter_crds_values_dropped_values: Counter,
    /// 过滤拉取响应
    pub(crate) filter_pull_response: Counter,
    /// 生成修剪消息
    pub(crate) generate_prune_messages: Counter,
    /// 生成拉取响应
    pub(crate) generate_pull_responses: Counter,
    /// 获取纪元持续时间
    pub(crate) get_epoch_duration_no_working_bank: Counter,
    /// 获取投票
    pub(crate) get_votes: Counter,
    /// 获取投票计数
    pub(crate) get_votes_count: Counter,
    /// 监听循环迭代次数
    pub(crate) gossip_listen_loop_iterations_since_last_report: Counter,
    /// 监听循环时间
    pub(crate) gossip_listen_loop_time: Counter,
    /// 丢弃的八卦包计数
    pub(crate) gossip_packets_dropped_count: Counter,
    /// 丢弃的拉取请求
    pub(crate) gossip_pull_request_dropped_requests: Counter,
    /// 拉取请求没有预算
    pub(crate) gossip_pull_request_no_budget: Counter,
    /// 发送拉取请求
    pub(crate) gossip_pull_request_sent_requests: Counter,
    /// 传输循环迭代次数
    pub(crate) gossip_transmit_loop_iterations_since_last_report: Counter,
    /// 传输循环时间
    pub(crate) gossip_transmit_loop_time: Counter,
    /// 处理批量ping消息时间
    pub(crate) handle_batch_ping_messages_time: Counter,
    /// 处理批量pong消息时间
    pub(crate) handle_batch_pong_messages_time: Counter,
    /// 处理批量修剪消息时间
    pub(crate) handle_batch_prune_messages_time: Counter,
    /// 处理批量拉取请求时间
    pub(crate) handle_batch_pull_requests_time: Counter,
    /// 处理批量拉取响应时间
    pub(crate) handle_batch_pull_responses_time: Counter,
    /// 处理批量推送消息时间
    pub(crate) handle_batch_push_messages_time: Counter,
    /// 新拉取请求
    pub(crate) new_pull_requests: Counter,
    /// 新拉取请求计数
    pub(crate) new_pull_requests_count: Counter,
    /// 新拉取请求ping计数
    pub(crate) new_pull_requests_pings_count: Counter,
    /// 新推送请求2
    pub(crate) new_push_requests2: Counter,
    /// 新推送请求
    pub(crate) new_push_requests: Counter,
    /// 新推送请求计数
    pub(crate) new_push_requests_num: Counter,
    /// 未验证的八卦地址计数
    pub(crate) num_unverifed_gossip_addrs: Counter,
    /// 接收到的包计数
    pub(crate) packets_received_count: Counter,
    /// 接收到的ping消息计数
    pub(crate) packets_received_ping_messages_count: Counter,
    /// 接收到的pong消息计数
    pub(crate) packets_received_pong_messages_count: Counter,
    /// 接收到的修剪消息计数
    pub(crate) packets_received_prune_messages_count: Counter,
    /// 接收到的拉取请求计数
    pub(crate) packets_received_pull_requests_count: Counter,
    /// 接收到的拉取响应计数
    pub(crate) packets_received_pull_responses_count: Counter,
    /// 接收到的推送消息计数
    pub(crate) packets_received_push_messages_count: Counter,
    /// 接收到的未知计数
    pub(crate) packets_received_unknown_count: Counter,
    /// 接收到的验证计数
    pub(crate) packets_received_verified_count: Counter,
    /// 发送的八卦请求计数
    pub(crate) packets_sent_gossip_requests_count: Counter,
    /// 发送的修剪消息计数
    pub(crate) packets_sent_prune_messages_count: Counter,
    /// 发送的拉取请求计数
    pub(crate) packets_sent_pull_requests_count: Counter,
    /// 发送的拉取响应计数
    pub(crate) packets_sent_pull_responses_count: Counter,
    /// 发送的推送消息计数
    pub(crate) packets_sent_push_messages_count: Counter,
    /// 处理八卦包时间
    pub(crate) process_gossip_packets_time: Counter,
    /// 处理修剪时间
    pub(crate) process_prune: Counter,
    /// 处理拉取响应时间
    pub(crate) process_pull_response: Counter,
    /// 处理拉取响应计数
    pub(crate) process_pull_response_count: Counter,
    /// 处理拉取响应失败插入
    pub(crate) process_pull_response_fail_insert: Counter,
    /// 处理拉取响应失败超时
    pub(crate) process_pull_response_fail_timeout: Counter,
    /// 处理拉取响应长度
    pub(crate) process_pull_response_len: Counter,
    /// 处理拉取响应成功
    pub(crate) process_pull_response_success: Counter,
    /// 处理推送消息
    pub(crate) process_push_message: Counter,
    /// 修剪消息计数
    pub(crate) prune_message_count: Counter,
    /// 修剪消息长度
    pub(crate) prune_message_len: Counter,
    /// 修剪消息超时
    pub(crate) prune_message_timeout: Counter,
    /// 修剪接收缓存
    pub(crate) prune_received_cache: Counter,
    /// 从入口点拉取计数
    pub(crate) pull_from_entrypoint_count: Counter,
    /// 拉取请求pingpong检查失败计数
    pub(crate) pull_request_ping_pong_check_failed_count: Counter,
    /// 拉取请求计数
    pub(crate) pull_requests_count: Counter,
    /// 清除
    pub(crate) purge: Counter,
    /// 清除计数
    pub(crate) purge_count: Counter,
    /// 推送扇出条目计数
    pub(crate) push_fanout_num_entries: Counter,
    /// 推送扇出节点计数
    pub(crate) push_fanout_num_nodes: Counter,
    /// 推送消息计数
    pub(crate) push_message_count: Counter,
    /// 推送消息推送计数
    pub(crate) push_message_pushes: Counter,
    /// 推送消息值计数
    pub(crate) push_message_value_count: Counter,
    /// 推送响应计数
    pub(crate) push_response_count: Counter,
    /// 推送投票读取
    pub(crate) push_vote_read: Counter,
    /// 修复对等体
    pub(crate) repair_peers: Counter,
    /// 需要 gossip 未知质押
    pub(crate) require_stake_for_gossip_unknown_stakes: Counter,
    /// 保存联系信息时间
    pub(crate) save_contact_info_time: Counter,
    /// 跳过拉取响应消息分片版本
    pub(crate) skip_pull_response_shred_version: Counter,
    /// 跳过拉取消息分片版本
    pub(crate) skip_pull_shred_version: Counter,
    /// 跳过推送消息分片版本
    pub(crate) skip_push_message_shred_version: Counter,
    /// 修剪crds表
    pub(crate) trim_crds_table: Counter,
    /// 修剪crds表失败
    pub(crate) trim_crds_table_failed: Counter,
    /// 修剪crds表已清除值计数
    pub(crate) trim_crds_table_purged_values_count: Counter,
    /// tvu节点
    pub(crate) tvu_peers: Counter,
    /// 验证八卦包时间
    pub(crate) verify_gossip_packets_time: Counter,
    /// 窗口请求回环
    pub(crate) window_request_loopback: Counter,
}

/// 发送统计信息
pub(crate) fn submit_gossip_stats(
    stats: &GossipStats,
    gossip: &CrdsGossip,
    stakes: &HashMap<Pubkey, u64>,
) {
    /// 从 crds 中获取
    let (crds_stats, table_size, num_nodes, num_pubkeys, purged_values_size, failed_inserts_size) = {
        let gossip_crds = gossip.crds.read().unwrap();
        (
            gossip_crds.take_stats(),
            gossip_crds.len(),
            gossip_crds.num_nodes(),
            gossip_crds.num_pubkeys(),
            gossip_crds.num_purged(),
            gossip.pull.failed_inserts_size(),
        )
    };
    let num_nodes_staked = stakes.values().filter(|stake| **stake > 0).count();
    datapoint_info!(
        "cluster_info_stats",
        ("entrypoint", stats.entrypoint.clear(), i64),
        ("entrypoint2", stats.entrypoint2.clear(), i64),
        ("push_vote_read", stats.push_vote_read.clear(), i64),
        ("get_votes", stats.get_votes.clear(), i64),
        ("get_votes_count", stats.get_votes_count.clear(), i64),
        ("all_tvu_peers", stats.all_tvu_peers.clear(), i64),
        ("tvu_peers", stats.tvu_peers.clear(), i64),
        (
            "new_push_requests_num",
            stats.new_push_requests_num.clear(),
            i64
        ),
        ("table_size", table_size as i64, i64),
        ("purged_values_size", purged_values_size as i64, i64),
        ("failed_inserts_size", failed_inserts_size as i64, i64),
        ("num_nodes", num_nodes as i64, i64),
        ("num_nodes_staked", num_nodes_staked as i64, i64),
        ("num_pubkeys", num_pubkeys, i64),
    );
    datapoint_info!(
        "cluster_info_stats2",
        (
            "gossip_packets_dropped_count",
            stats.gossip_packets_dropped_count.clear(),
            i64
        ),
        ("repair_peers", stats.repair_peers.clear(), i64),
        ("new_push_requests", stats.new_push_requests.clear(), i64),
        ("new_push_requests2", stats.new_push_requests2.clear(), i64),
        ("purge", stats.purge.clear(), i64),
        ("purge_count", stats.purge_count.clear(), i64),
        (
            "process_gossip_packets_time",
            stats.process_gossip_packets_time.clear(),
            i64
        ),
        (
            "verify_gossip_packets_time",
            stats.verify_gossip_packets_time.clear(),
            i64
        ),
        (
            "handle_batch_ping_messages_time",
            stats.handle_batch_ping_messages_time.clear(),
            i64
        ),
        (
            "handle_batch_pong_messages_time",
            stats.handle_batch_pong_messages_time.clear(),
            i64
        ),
        (
            "handle_batch_prune_messages_time",
            stats.handle_batch_prune_messages_time.clear(),
            i64
        ),
        (
            "handle_batch_pull_requests_time",
            stats.handle_batch_pull_requests_time.clear(),
            i64
        ),
        (
            "handle_batch_pull_responses_time",
            stats.handle_batch_pull_responses_time.clear(),
            i64
        ),
        (
            "handle_batch_push_messages_time",
            stats.handle_batch_push_messages_time.clear(),
            i64
        ),
        (
            "process_pull_resp",
            stats.process_pull_response.clear(),
            i64
        ),
        ("filter_pull_resp", stats.filter_pull_response.clear(), i64),
        (
            "filter_crds_values_dropped_requests",
            stats.filter_crds_values_dropped_requests.clear(),
            i64
        ),
        (
            "filter_crds_values_dropped_values",
            stats.filter_crds_values_dropped_values.clear(),
            i64
        ),
        (
            "process_pull_resp_count",
            stats.process_pull_response_count.clear(),
            i64
        ),
        (
            "pull_response_fail_insert",
            stats.process_pull_response_fail_insert.clear(),
            i64
        ),
        (
            "pull_response_fail_timeout",
            stats.process_pull_response_fail_timeout.clear(),
            i64
        ),
        (
            "pull_response_success",
            stats.process_pull_response_success.clear(),
            i64
        ),
        (
            "num_redundant_pull_responses",
            crds_stats.num_redundant_pull_responses,
            i64
        ),
        (
            "push_response_count",
            stats.push_response_count.clear(),
            i64
        ),
        (
            "save_contact_info_time",
            stats.save_contact_info_time.clear(),
            i64
        ),
    );
    datapoint_info!(
        "cluster_info_stats3",
        (
            "process_pull_resp_len",
            stats.process_pull_response_len.clear(),
            i64
        ),
        (
            "pull_request_ping_pong_check_failed_count",
            stats.pull_request_ping_pong_check_failed_count.clear(),
            i64
        ),
        (
            "new_pull_requests_pings_count",
            stats.new_pull_requests_pings_count.clear(),
            i64
        ),
        (
            "generate_pull_responses",
            stats.generate_pull_responses.clear(),
            i64
        ),
        ("process_prune", stats.process_prune.clear(), i64),
        (
            "prune_message_timeout",
            stats.prune_message_timeout.clear(),
            i64
        ),
        (
            "bad_prune_destination",
            stats.bad_prune_destination.clear(),
            i64
        ),
        (
            "process_push_message",
            stats.process_push_message.clear(),
            i64
        ),
        (
            "prune_received_cache",
            stats.prune_received_cache.clear(),
            i64
        ),
        ("epoch_slots_lookup", stats.epoch_slots_lookup.clear(), i64),
        ("new_pull_requests", stats.new_pull_requests.clear(), i64),
        (
            "gossip_pull_request_no_budget",
            stats.gossip_pull_request_no_budget.clear(),
            i64
        ),
        (
            "gossip_pull_request_sent_requests",
            stats.gossip_pull_request_sent_requests.clear(),
            i64
        ),
        (
            "gossip_pull_request_dropped_requests",
            stats.gossip_pull_request_dropped_requests.clear(),
            i64
        ),
        (
            "gossip_transmit_loop_time",
            stats.gossip_transmit_loop_time.clear(),
            i64
        ),
        (
            "gossip_transmit_loop_iterations_since_last_report",
            stats
                .gossip_transmit_loop_iterations_since_last_report
                .clear(),
            i64
        ),
        (
            "gossip_listen_loop_time",
            stats.gossip_listen_loop_time.clear(),
            i64
        ),
        (
            "gossip_listen_loop_iterations_since_last_report",
            stats
                .gossip_listen_loop_iterations_since_last_report
                .clear(),
            i64
        ),
    );
    datapoint_info!(
        "cluster_info_stats4",
        (
            "skip_push_message_shred_version",
            stats.skip_push_message_shred_version.clear(),
            i64
        ),
        (
            "skip_pull_response_shred_version",
            stats.skip_pull_response_shred_version.clear(),
            i64
        ),
        (
            "skip_pull_shred_version",
            stats.skip_pull_shred_version.clear(),
            i64
        ),
        ("push_message_count", stats.push_message_count.clear(), i64),
        (
            "num_duplicate_push_messages",
            crds_stats.num_duplicate_push_messages,
            i64
        ),
        (
            "push_fanout_num_entries",
            stats.push_fanout_num_entries.clear(),
            i64
        ),
        (
            "push_fanout_num_nodes",
            stats.push_fanout_num_nodes.clear(),
            i64
        ),
        (
            "push_message_pushes",
            stats.push_message_pushes.clear(),
            i64
        ),
        (
            "push_message_value_count",
            stats.push_message_value_count.clear(),
            i64
        ),
        (
            "new_pull_requests_count",
            stats.new_pull_requests_count.clear(),
            i64
        ),
        (
            "pull_from_entrypoint_count",
            stats.pull_from_entrypoint_count.clear(),
            i64
        ),
        (
            "prune_message_count",
            stats.prune_message_count.clear(),
            i64
        ),
        ("prune_message_len", stats.prune_message_len.clear(), i64),
        ("epoch_slots_filled", stats.epoch_slots_filled.clear(), i64),
        (
            "window_request_loopback",
            stats.window_request_loopback.clear(),
            i64
        ),
        (
            "get_epoch_duration_no_working_bank",
            stats.get_epoch_duration_no_working_bank.clear(),
            i64
        ),
        (
            "generate_prune_messages",
            stats.generate_prune_messages.clear(),
            i64
        ),
    );
    datapoint_info!(
        "cluster_info_stats5",
        (
            "pull_requests_count",
            stats.pull_requests_count.clear(),
            i64
        ),
        (
            "num_unverifed_gossip_addrs",
            stats.num_unverifed_gossip_addrs.clear(),
            i64
        ),
        (
            "packets_received_count",
            stats.packets_received_count.clear(),
            i64
        ),
        (
            "packets_received_ping_messages_count",
            stats.packets_received_ping_messages_count.clear(),
            i64
        ),
        (
            "packets_received_pong_messages_count",
            stats.packets_received_pong_messages_count.clear(),
            i64
        ),
        (
            "packets_received_prune_messages_count",
            stats.packets_received_prune_messages_count.clear(),
            i64
        ),
        (
            "packets_received_pull_requests_count",
            stats.packets_received_pull_requests_count.clear(),
            i64
        ),
        (
            "packets_received_pull_responses_count",
            stats.packets_received_pull_responses_count.clear(),
            i64
        ),
        (
            "packets_received_push_messages_count",
            stats.packets_received_push_messages_count.clear(),
            i64
        ),
        (
            "packets_received_unknown_count",
            stats.packets_received_unknown_count.clear(),
            i64
        ),
        (
            "packets_received_verified_count",
            stats.packets_received_verified_count.clear(),
            i64
        ),
        (
            "packets_sent_gossip_requests_count",
            stats.packets_sent_gossip_requests_count.clear(),
            i64
        ),
        (
            "packets_sent_prune_messages_count",
            stats.packets_sent_prune_messages_count.clear(),
            i64
        ),
        (
            "packets_sent_pull_requests_count",
            stats.packets_sent_pull_requests_count.clear(),
            i64
        ),
        (
            "packets_sent_pull_responses_count",
            stats.packets_sent_pull_responses_count.clear(),
            i64
        ),
        (
            "packets_sent_push_messages_count",
            stats.packets_sent_push_messages_count.clear(),
            i64
        ),
        (
            "require_stake_for_gossip_unknown_stakes",
            stats.require_stake_for_gossip_unknown_stakes.clear(),
            i64
        ),
        ("trim_crds_table", stats.trim_crds_table.clear(), i64),
        (
            "trim_crds_table_failed",
            stats.trim_crds_table_failed.clear(),
            i64
        ),
        (
            "trim_crds_table_purged_values_count",
            stats.trim_crds_table_purged_values_count.clear(),
            i64
        ),
    );
    datapoint_info!(
        "cluster_info_crds_stats",
        ("LegacyContactInfo-push", crds_stats.push.counts[0], i64),
        ("LegacyContactInfo-pull", crds_stats.pull.counts[0], i64),
        ("Vote-push", crds_stats.push.counts[1], i64),
        ("Vote-pull", crds_stats.pull.counts[1], i64),
        ("LowestSlot-push", crds_stats.push.counts[2], i64),
        ("LowestSlot-pull", crds_stats.pull.counts[2], i64),
        ("LegacySnapshotHashes-push", crds_stats.push.counts[3], i64),
        ("LegacySnapshotHashes-pull", crds_stats.pull.counts[3], i64),
        ("AccountsHashes-push", crds_stats.push.counts[4], i64),
        ("AccountsHashes-pull", crds_stats.pull.counts[4], i64),
        ("EpochSlots-push", crds_stats.push.counts[5], i64),
        ("EpochSlots-pull", crds_stats.pull.counts[5], i64),
        ("LegacyVersion-push", crds_stats.push.counts[6], i64),
        ("LegacyVersion-pull", crds_stats.pull.counts[6], i64),
        ("Version-push", crds_stats.push.counts[7], i64),
        ("Version-pull", crds_stats.pull.counts[7], i64),
        ("NodeInstance-push", crds_stats.push.counts[8], i64),
        ("NodeInstance-pull", crds_stats.pull.counts[8], i64),
        ("DuplicateShred-push", crds_stats.push.counts[9], i64),
        ("DuplicateShred-pull", crds_stats.pull.counts[9], i64),
        ("SnapshotHashes-push", crds_stats.push.counts[10], i64),
        ("SnapshotHashes-pull", crds_stats.pull.counts[10], i64),
        ("ContactInfo-push", crds_stats.push.counts[11], i64),
        ("ContactInfo-pull", crds_stats.pull.counts[11], i64),
        (
            "RestartLastVotedForkSlots-push",
            crds_stats.push.counts[12],
            i64
        ),
        (
            "RestartLastVotedForkSlots-pull",
            crds_stats.pull.counts[12],
            i64
        ),
        ("RestartHeaviestFork-push", crds_stats.push.counts[13], i64),
        ("RestartHeaviestFork-pull", crds_stats.pull.counts[13], i64),
        (
            "all-push",
            crds_stats.push.counts.iter().sum::<usize>(),
            i64
        ),
        (
            "all-pull",
            crds_stats.pull.counts.iter().sum::<usize>(),
            i64
        ),
    );
    datapoint_info!(
        "cluster_info_crds_stats_fails",
        ("LegacyContactInfo-push", crds_stats.push.fails[0], i64),
        ("LegacyContactInfo-pull", crds_stats.pull.fails[0], i64),
        ("Vote-push", crds_stats.push.fails[1], i64),
        ("Vote-pull", crds_stats.pull.fails[1], i64),
        ("LowestSlot-push", crds_stats.push.fails[2], i64),
        ("LowestSlot-pull", crds_stats.pull.fails[2], i64),
        ("LegacySnapshotHashes-push", crds_stats.push.fails[3], i64),
        ("LegacySnapshotHashes-pull", crds_stats.pull.fails[3], i64),
        ("AccountsHashes-push", crds_stats.push.fails[4], i64),
        ("AccountsHashes-pull", crds_stats.pull.fails[4], i64),
        ("EpochSlots-push", crds_stats.push.fails[5], i64),
        ("EpochSlots-pull", crds_stats.pull.fails[5], i64),
        ("LegacyVersion-push", crds_stats.push.fails[6], i64),
        ("LegacyVersion-pull", crds_stats.pull.fails[6], i64),
        ("Version-push", crds_stats.push.fails[7], i64),
        ("Version-pull", crds_stats.pull.fails[7], i64),
        ("NodeInstance-push", crds_stats.push.fails[8], i64),
        ("NodeInstance-pull", crds_stats.pull.fails[8], i64),
        ("DuplicateShred-push", crds_stats.push.fails[9], i64),
        ("DuplicateShred-pull", crds_stats.pull.fails[9], i64),
        ("SnapshotHashes-push", crds_stats.push.fails[10], i64),
        ("SnapshotHashes-pull", crds_stats.pull.fails[10], i64),
        ("ContactInfo-push", crds_stats.push.fails[11], i64),
        ("ContactInfo-pull", crds_stats.pull.fails[11], i64),
        (
            "RestartLastVotedForkSlots-push",
            crds_stats.push.fails[12],
            i64
        ),
        (
            "RestartLastVotedForkSlots-pull",
            crds_stats.pull.fails[12],
            i64
        ),
        ("RestartHeaviestFork-push", crds_stats.push.fails[13], i64),
        ("RestartHeaviestFork-pull", crds_stats.pull.fails[13], i64),
        ("all-push", crds_stats.push.fails.iter().sum::<usize>(), i64),
        ("all-pull", crds_stats.pull.fails.iter().sum::<usize>(), i64),
    );
    if !log::log_enabled!(log::Level::Trace) {
        return;
    }
    submit_vote_stats("cluster_info_crds_stats_votes_pull", &crds_stats.pull.votes);
    submit_vote_stats("cluster_info_crds_stats_votes_push", &crds_stats.push.votes);
    let votes: HashMap<Slot, usize> = crds_stats
        .pull
        .votes
        .into_iter()
        .chain(crds_stats.push.votes)
        .into_grouping_map()
        .aggregate(|acc, _slot, num_votes| Some(acc.unwrap_or_default() + num_votes));
    submit_vote_stats("cluster_info_crds_stats_votes", &votes);
}

fn submit_vote_stats<'a, I>(name: &'static str, votes: I)
where
    I: IntoIterator<Item = (&'a Slot, /*num-votes:*/ &'a usize)>,
{
    // Submit vote stats only for the top most voted slots.
    const NUM_SLOTS: usize = 10;
    let mut votes: Vec<_> = votes.into_iter().map(|(k, v)| (*k, *v)).collect();
    if votes.len() > NUM_SLOTS {
        votes.select_nth_unstable_by_key(NUM_SLOTS, |(_, num)| Reverse(*num));
    }
    for (slot, num_votes) in votes.into_iter().take(NUM_SLOTS) {
        datapoint_trace!(name, ("slot", slot, i64), ("num_votes", num_votes, i64));
    }
}
