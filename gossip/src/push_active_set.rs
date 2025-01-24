use {
    crate::weighted_shuffle::WeightedShuffle,
    indexmap::IndexMap,
    rand::Rng,
    solana_bloom::bloom::{Bloom, ConcurrentBloom},
    solana_sdk::{native_token::LAMPORTS_PER_SOL, pubkey::Pubkey},
    std::collections::HashMap,
};

/// 推送集的条目数，每个条目都是个质押桶，所以是质押桶数
const NUM_PUSH_ACTIVE_SET_ENTRIES: usize = 25;

// Each entry corresponds to a stake bucket for
//     min stake of { this node, crds value owner }
// The entry represents set of gossip nodes to actively
// push to for crds values belonging to the bucket.
/// 这些节点集与 CRDS 值所有者的最小质押相关，并表示一组应该主动向其推送 CRDS 值的节点。
/// 是一个质押桶列表
/// 索引是质押量的sol位数，0位数放索引0，1位数放索引1，...
#[derive(Default)]
pub(crate) struct PushActiveSet([PushActiveSetEntry; NUM_PUSH_ACTIVE_SET_ENTRIES]);

// Keys are gossip nodes to push messages to.
// Values are which origins the node has pruned.
/// 质押桶，每个质押桶里放的都是质押量的sol位数一样的节点，
/// 比如都质押了十位数的sol，那就在一个桶里，都质押了百位数的在同一个桶里
/// 键是八卦应该发送到的节点公钥
/// 值是布隆过滤器，里面包含的是一个应该已经知道这条消息的节点的集合，比如是这条消息的来源
/// 布隆过滤器可以快速判断一个值是不是在集合里，有一定概率把不在集合里的说成在集合里，不过一旦在集合里就一定会说在集合里
/// 这样发送可以过滤掉已经知道这条消息的节点
#[derive(Default)]
struct PushActiveSetEntry(IndexMap</*node:*/ Pubkey, /*origins:*/ ConcurrentBloom<Pubkey>>);

impl PushActiveSet {
    /// 最小布隆项数
    #[cfg(debug_assertions)]
    const MIN_NUM_BLOOM_ITEMS: usize = 512;
    /// 最小布隆项数
    #[cfg(not(debug_assertions))]
    const MIN_NUM_BLOOM_ITEMS: usize = crate::cluster_info::CRDS_UNIQUE_PUBKEY_CAPACITY;

    /// 取需要推送的节点
    pub(crate) fn get_nodes<'a>(
        &'a self,
        pubkey: &'a Pubkey, // This node.
        origin: &'a Pubkey, // CRDS value owner.
        // If true forces gossip push even if the node has pruned the origin.
        should_force_push: impl FnMut(&Pubkey) -> bool + 'a,
        stakes: &HashMap<Pubkey, u64>,
    ) -> impl Iterator<Item = &'a Pubkey> + 'a {
        /// 取自己以及crds值来源的节点的最小值
        let stake = stakes.get(pubkey).min(stakes.get(origin));
        /// 取这个数量级的质押桶
        self.get_entry(stake)
             /// 从桶里面取
            .get_nodes(pubkey, origin, should_force_push)
    }

    // Prunes origins for the given gossip node.
    // We will stop pushing messages from the specified origins to the node.
    /// 向布隆过滤器添加某个节点，即停止推送这些来源的消息
    pub(crate) fn prune(
        &self,
        pubkey: &Pubkey,    // This node.
        node: &Pubkey,      // Gossip node.
        origins: &[Pubkey], // CRDS value owners.
        stakes: &HashMap<Pubkey, u64>,
    ) {
        /// 自己的质押量
        let stake = stakes.get(pubkey);
        for origin in origins {
            /// 不过滤自己
            if origin == pubkey {
                continue;
            }
            /// 自己质押量与待过滤质押量的最小值
            let stake = stake.min(stakes.get(origin));
            /// 向布隆过滤器添加
            self.get_entry(stake).prune(node, origin)
        }
    }

    /// 加一批新的节点进来，然后重新洗牌，丢掉一些
    pub(crate) fn rotate<R: Rng>(
        &mut self,
        rng: &mut R,
        size: usize, // Number of nodes to retain in each active-set entry.
        cluster_size: usize,
        // Gossip nodes to be sampled for each push active set.
        nodes: &[Pubkey],
        stakes: &HashMap<Pubkey, u64>,
    ) {
        /// 集群大小和最小布隆项数取最小值
        let num_bloom_filter_items = cluster_size.max(Self::MIN_NUM_BLOOM_ITEMS);
        // Active set of nodes to push to are sampled from these gossip nodes,
        // using sampling probabilities obtained from the stake bucket of each
        // node.
        /// 取这些节点的桶的质押量的位数
        let buckets: Vec<_> = nodes
            .iter()
            .map(|node| get_stake_bucket(stakes.get(node)))
            .collect();
        // (k, entry) represents push active set where the stake bucket of
        //     min stake of {this node, crds value owner}
        // is equal to `k`. The `entry` maintains set of gossip nodes to
        // actively push to for crds values belonging to this bucket.
        for (k, entry) in self.0.iter_mut().enumerate() {
            /// 计算权重，位数 + 1 再取平方，应该是增大洗牌的时候的区分度
            let weights: Vec<u64> = buckets
                .iter()
                .map(|&bucket| {
                    // bucket <- get_stake_bucket(min stake of {
                    //  this node, crds value owner and gossip peer
                    // })
                    // weight <- (bucket + 1)^2
                    // min stake of {...} is a proxy for how much we care about
                    // the link, and tries to mirror similar logic on the
                    // receiving end when pruning incoming links:
                    // https://github.com/solana-labs/solana/blob/81394cf92/gossip/src/received_cache.rs#L100-L105
                    let bucket = bucket.min(k) as u64;
                    bucket.saturating_add(1).saturating_pow(2)
                })
                .collect();
            /// 桶内洗牌
            entry.rotate(rng, size, num_bloom_filter_items, nodes, &weights);
        }
    }

    /// 取某个数量级质押量的质押桶
    fn get_entry(&self, stake: Option<&u64>) -> &PushActiveSetEntry {
        &self.0[get_stake_bucket(stake)]
    }
}

impl PushActiveSetEntry {
    /// 布隆失败率，用来配置新建的布隆过滤器
    const BLOOM_FALSE_RATE: f64 = 0.1;
    /// 布隆最大比特，用来配置新建的布隆过滤器
    const BLOOM_MAX_BITS: usize = 1024 * 8 * 4;

    /// 从桶里取要发送的节点，排除掉已经知道的节点
    fn get_nodes<'a>(
        &'a self,
        pubkey: &'a Pubkey, // This node.
        origin: &'a Pubkey, // CRDS value owner.
        // If true forces gossip push even if the node has pruned the origin.
        mut should_force_push: impl FnMut(&Pubkey) -> bool + 'a,
    ) -> impl Iterator<Item = &'a Pubkey> + 'a {
        let pubkey_eq_origin = pubkey == origin;
        self.0
            .iter()
            .filter(move |(node, bloom_filter)| {
                // Bloom filter can return false positive for origin == pubkey
                // but a node should always be able to push its own values.
                /// 已经知道这条crds的节点不用发（布隆过滤器有可能把不知道当成知道
                !bloom_filter.contains(origin)
                    /// 来源是自己，目的不是自己的要发
                    || (pubkey_eq_origin && &pubkey != node)
                    /// 开启强制发送的要发
                    || should_force_push(node)
            })
            .map(|(node, _bloom_filter)| node)
    }

    /// 向布隆过滤器里增加值
    fn prune(
        &self,
        node: &Pubkey,   // Gossip node.
        origin: &Pubkey, // CRDS value owner
    ) {
        if let Some(bloom_filter) = self.0.get(node) {
            bloom_filter.add(origin);
        }
    }

    /// 加一批新的节点进来，然后重新洗牌，丢掉一些
    fn rotate<R: Rng>(
        &mut self,
        rng: &mut R,
        size: usize, // Number of nodes to retain.
        num_bloom_filter_items: usize,
        nodes: &[Pubkey],
        weights: &[u64],
    ) {
        /// 权重和节点一样多
        debug_assert_eq!(nodes.len(), weights.len());
        /// 都有权重且不为零
        debug_assert!(weights.iter().all(|&weight| weight != 0u64));
        /// 洗牌，权重越大的指标出现的越早，与其权重成正比。
        let shuffle = WeightedShuffle::new("rotate-active-set", weights).shuffle(rng);
        for node in shuffle.map(|k| &nodes[k]) {
            // We intend to discard the oldest/first entry in the index-map.
            /// 超过期望保留长度就不再加了
            if self.0.len() > size {
                break;
            }
            /// 目标地址里已经有了的
            if self.0.contains_key(node) {
                continue;
            }
            /// 给这个质押项新建布隆过滤器
            let bloom = ConcurrentBloom::from(Bloom::random(
                num_bloom_filter_items,
                Self::BLOOM_FALSE_RATE,
                Self::BLOOM_MAX_BITS,
            ));
            /// 把目的和源一样的加到过滤器里
            bloom.add(node);
            /// 把质押项插进质押桶里
            self.0.insert(*node, bloom);
        }
        // Drop the oldest entry while preserving the ordering of others.
        /// 把最老的项扔掉
        while self.0.len() > size {
            self.0.shift_remove_index(0);
        }
    }
}

// Maps stake to bucket index.
/// 质押量对应的桶索引，是看sol质押的位数，0位数放索引0，1位数放索引1，...
fn get_stake_bucket(stake: Option<&u64>) -> usize {
    /// 获取质押的 sol 数
    let stake = stake.copied().unwrap_or_default() / LAMPORTS_PER_SOL;
    /// 获取质押了几位数的 sol，用 u64 的比特数减去质押的前导零的个数
    let bucket = u64::BITS - stake.leading_zeros();
    /// 超过索引大小的就都放在最后一个元素里
    (bucket as usize).min(NUM_PUSH_ACTIVE_SET_ENTRIES - 1)
}

#[cfg(test)]
mod tests {
    use {
        super::*, itertools::iproduct, rand::SeedableRng, rand_chacha::ChaChaRng,
        std::iter::repeat_with,
    };

    #[test]
    fn test_get_stake_bucket() {
        assert_eq!(get_stake_bucket(None), 0);
        let buckets = [0, 1, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5];
        for (k, bucket) in buckets.into_iter().enumerate() {
            let stake = (k as u64) * LAMPORTS_PER_SOL;
            assert_eq!(get_stake_bucket(Some(&stake)), bucket);
        }
        for (stake, bucket) in [
            (4_194_303, 22),
            (4_194_304, 23),
            (8_388_607, 23),
            (8_388_608, 24),
        ] {
            let stake = stake * LAMPORTS_PER_SOL;
            assert_eq!(get_stake_bucket(Some(&stake)), bucket);
        }
        assert_eq!(
            get_stake_bucket(Some(&u64::MAX)),
            NUM_PUSH_ACTIVE_SET_ENTRIES - 1
        );
    }

    #[test]
    fn test_push_active_set() {
        const CLUSTER_SIZE: usize = 117;
        const MAX_STAKE: u64 = (1 << 20) * LAMPORTS_PER_SOL;
        let mut rng = ChaChaRng::from_seed([189u8; 32]);
        let pubkey = Pubkey::new_unique();
        let nodes: Vec<_> = repeat_with(Pubkey::new_unique).take(20).collect();
        let stakes = repeat_with(|| rng.gen_range(1..MAX_STAKE));
        let mut stakes: HashMap<_, _> = nodes.iter().copied().zip(stakes).collect();
        stakes.insert(pubkey, rng.gen_range(1..MAX_STAKE));
        let mut active_set = PushActiveSet::default();
        assert!(active_set.0.iter().all(|entry| entry.0.is_empty()));
        active_set.rotate(&mut rng, 5, CLUSTER_SIZE, &nodes, &stakes);
        assert!(active_set.0.iter().all(|entry| entry.0.len() == 5));
        // Assert that for all entries, each filter already prunes the key.
        for entry in &active_set.0 {
            for (node, filter) in entry.0.iter() {
                assert!(filter.contains(node));
            }
        }
        let other = &nodes[5];
        let origin = &nodes[17];
        assert!(active_set
            .get_nodes(&pubkey, origin, |_| false, &stakes)
            .eq([13, 5, 18, 16, 0].into_iter().map(|k| &nodes[k])));
        assert!(active_set
            .get_nodes(&pubkey, other, |_| false, &stakes)
            .eq([13, 18, 16, 0].into_iter().map(|k| &nodes[k])));
        active_set.prune(&pubkey, &nodes[5], &[*origin], &stakes);
        active_set.prune(&pubkey, &nodes[3], &[*origin], &stakes);
        active_set.prune(&pubkey, &nodes[16], &[*origin], &stakes);
        assert!(active_set
            .get_nodes(&pubkey, origin, |_| false, &stakes)
            .eq([13, 18, 0].into_iter().map(|k| &nodes[k])));
        assert!(active_set
            .get_nodes(&pubkey, other, |_| false, &stakes)
            .eq([13, 18, 16, 0].into_iter().map(|k| &nodes[k])));
        active_set.rotate(&mut rng, 7, CLUSTER_SIZE, &nodes, &stakes);
        assert!(active_set.0.iter().all(|entry| entry.0.len() == 7));
        assert!(active_set
            .get_nodes(&pubkey, origin, |_| false, &stakes)
            .eq([18, 0, 7, 15, 11].into_iter().map(|k| &nodes[k])));
        assert!(active_set
            .get_nodes(&pubkey, other, |_| false, &stakes)
            .eq([18, 16, 0, 7, 15, 11].into_iter().map(|k| &nodes[k])));
        let origins = [*origin, *other];
        active_set.prune(&pubkey, &nodes[18], &origins, &stakes);
        active_set.prune(&pubkey, &nodes[0], &origins, &stakes);
        active_set.prune(&pubkey, &nodes[15], &origins, &stakes);
        assert!(active_set
            .get_nodes(&pubkey, origin, |_| false, &stakes)
            .eq([7, 11].into_iter().map(|k| &nodes[k])));
        assert!(active_set
            .get_nodes(&pubkey, other, |_| false, &stakes)
            .eq([16, 7, 11].into_iter().map(|k| &nodes[k])));
    }

    #[test]
    fn test_push_active_set_entry() {
        const NUM_BLOOM_FILTER_ITEMS: usize = 100;
        let mut rng = ChaChaRng::from_seed([147u8; 32]);
        let nodes: Vec<_> = repeat_with(Pubkey::new_unique).take(20).collect();
        let weights: Vec<_> = repeat_with(|| rng.gen_range(1..1000)).take(20).collect();
        let mut entry = PushActiveSetEntry::default();
        entry.rotate(
            &mut rng,
            5, // size
            NUM_BLOOM_FILTER_ITEMS,
            &nodes,
            &weights,
        );
        assert_eq!(entry.0.len(), 5);
        let keys = [&nodes[16], &nodes[11], &nodes[17], &nodes[14], &nodes[5]];
        assert!(entry.0.keys().eq(keys));
        for (pubkey, origin) in iproduct!(&nodes, &nodes) {
            if !keys.contains(&origin) {
                assert!(entry.get_nodes(pubkey, origin, |_| false).eq(keys));
            } else {
                assert!(entry.get_nodes(pubkey, origin, |_| true).eq(keys));
                assert!(entry
                    .get_nodes(pubkey, origin, |_| false)
                    .eq(keys.into_iter().filter(|&key| key != origin)));
            }
        }
        // Assert that each filter already prunes the key.
        for (node, filter) in entry.0.iter() {
            assert!(filter.contains(node));
        }
        for (pubkey, origin) in iproduct!(&nodes, keys) {
            assert!(entry.get_nodes(pubkey, origin, |_| true).eq(keys));
            assert!(entry
                .get_nodes(pubkey, origin, |_| false)
                .eq(keys.into_iter().filter(|&node| node != origin)));
        }
        // Assert that prune excludes node from get.
        let origin = &nodes[3];
        entry.prune(&nodes[11], origin);
        entry.prune(&nodes[14], origin);
        entry.prune(&nodes[19], origin);
        for pubkey in &nodes {
            assert!(entry.get_nodes(pubkey, origin, |_| true).eq(keys));
            assert!(entry.get_nodes(pubkey, origin, |_| false).eq(keys
                .into_iter()
                .filter(|&&node| pubkey == origin || (node != nodes[11] && node != nodes[14]))));
        }
        // Assert that rotate adds new nodes.
        entry.rotate(&mut rng, 5, NUM_BLOOM_FILTER_ITEMS, &nodes, &weights);
        let keys = [&nodes[11], &nodes[17], &nodes[14], &nodes[5], &nodes[7]];
        assert!(entry.0.keys().eq(keys));
        entry.rotate(&mut rng, 6, NUM_BLOOM_FILTER_ITEMS, &nodes, &weights);
        let keys = [
            &nodes[17], &nodes[14], &nodes[5], &nodes[7], &nodes[1], &nodes[13],
        ];
        assert!(entry.0.keys().eq(keys));
        entry.rotate(&mut rng, 4, NUM_BLOOM_FILTER_ITEMS, &nodes, &weights);
        let keys = [&nodes[5], &nodes[7], &nodes[1], &nodes[13]];
        assert!(entry.0.keys().eq(keys));
    }
}
