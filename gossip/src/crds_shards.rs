use {
    crate::{crds::VersionedCrdsValue, crds_gossip_pull::CrdsFilter},
    indexmap::map::IndexMap,
    std::{
        cmp::Ordering,
        ops::{Index, IndexMut},
    },
};

/// 由 crds 构成的crds哈希前导分组列表
/// 为的是把 crds 值放进去以后按哈希的前导位进行聚合，后面可以按哈希前导位比特值快速取出符合的 crds 值
/// 这里主要只记录它的索引，而不是值。看成是用IndexSet而不是IndexMap也行，主要用的是key。
/// 一个crds哈希前导分组里可以插入许多的 crds 值，作为一个IndexMap，多个crds哈希前导分组构成这个crds哈希前导分组列表
/// 这个就是crds哈希前导分组的列表，也就是crds分两层存放在这个结构中 shards列表（shard（crds值））
/// 第一层是一个数组，每个位置上放的是哈希值前shard_bits位等于位置索引的一组 crds 值，这一组值叫做一个 crds哈希前导分组，整个数组叫做crds哈希前导分组列表
/// 哈希值[[1 0 1 0]10101001]每位可以有0和1两种值，所以可能会被放在2 ^ shard_bits个可能的位置上，所以数组也要这么长
/// 每个分片是一个 IndexMap，它将 CRDS 值的索引（usize）映射到它的哈希值（u64）
#[derive(Clone)]
pub struct CrdsShards {
    // shards[k] includes crds values which the first shard_bits of their hash
    // value is equal to k. Each shard is a mapping from crds values indices to
    // their hash value.
    /// 每个元素里都是聚合的前shard_bits位相同的crds值
    shards: Vec<IndexMap<usize, u64>>,
    /// crds哈希分组的前导位数
    shard_bits: u32,
}

impl CrdsShards {
    /// 新建crds哈希前导分组列表，指定前导位数
    pub fn new(shard_bits: u32) -> Self {
        CrdsShards {
            shards: vec![IndexMap::new(); 1 << shard_bits],
            shard_bits,
        }
    }

    /// 插入 crds 值
    pub fn insert(&mut self, index: usize, value: &VersionedCrdsValue) -> bool {
        /// 计算哈希
        let hash = CrdsFilter::hash_as_u64(value.value.hash());
        /// 根据哈希前导位数选择一个 crds哈希前导分组，插入其中
        self.shard_mut(hash).insert(index, hash).is_none()
    }

    /// 移除 crds 值
    pub fn remove(&mut self, index: usize, value: &VersionedCrdsValue) -> bool {
        /// 计算哈希
        let hash = CrdsFilter::hash_as_u64(value.value.hash());
        /// 根据哈希前导位数选择一个 crds哈希前导分组，从中移除这个值
        self.shard_mut(hash).swap_remove(&index).is_some()
    }

    /// Returns indices of all crds values which the first 'mask_bits' of their
    /// hash value is equal to 'mask'.
    /// 找到指定前导位数比特值的所有crds哈希前导分组，然后取它们的 crds 值的索引出来
    pub fn find(&self, mask: u64, mask_bits: u32) -> impl Iterator<Item = usize> + '_ {
        let ones = (!0u64).checked_shr(mask_bits).unwrap_or(0);
        let mask = mask | ones;
        /// 比较前导长度，分别装在不同的迭代器枚举类型里
        match self.shard_bits.cmp(&mask_bits) {
            /// 指定的mask更大，除了找到合适的crds哈希前导分组外，还要额外过滤几位的
            Ordering::Less => {
                let pred = move |(&index, hash)| {
                    if hash | ones == mask {
                        Some(index)
                    } else {
                        None
                    }
                };
                Iter::Less(self.shard(mask).iter().filter_map(pred))
            }
            /// 相等，那就直接取那里的crds哈希前导分组里的 crds 就行了
            Ordering::Equal => Iter::Equal(self.shard(mask).keys().cloned()),
            /// 指定的mask更小，还好是数组按顺序放的，这样可以找到多个区间的 crds哈希前导分组一起 flat 取出来了
            Ordering::Greater => {
                let count = 1 << (self.shard_bits - mask_bits);
                let end = self.shard_index(mask) + 1;
                Iter::Greater(
                    self.shards[end - count..end]
                        .iter()
                        .flat_map(IndexMap::keys)
                        .cloned(),
                )
            }
        }
    }

    /// 通过 crds 值的哈希值的前 shard_bits 位来确定 crds 值所属的分片
    #[inline]
    fn shard_index(&self, hash: u64) -> usize {
        hash.checked_shr(64 - self.shard_bits).unwrap_or(0) as usize
    }

    /// 根据 crds 值的哈希，在crds哈希前导分组列表里找到一个crds哈希前导分组，取它的共享引用
    #[inline]
    fn shard(&self, hash: u64) -> &IndexMap<usize, u64> {
        let shard_index = self.shard_index(hash);
        self.shards.index(shard_index)
    }

    /// 根据 crds 值的哈希，在crds哈希前导分组列表里找到一个crds哈希前导分组，取它的可变引用
    #[inline]
    fn shard_mut(&mut self, hash: u64) -> &mut IndexMap<usize, u64> {
        let shard_index = self.shard_index(hash);
        self.shards.index_mut(shard_index)
    }

    // Checks invariants in the shards tables against the crds table.
    /// 测试用
    #[cfg(test)]
    pub fn check(&self, crds: &[VersionedCrdsValue]) {
        let mut indices: Vec<_> = self
            .shards
            .iter()
            .flat_map(IndexMap::keys)
            .cloned()
            .collect();
        indices.sort_unstable();
        assert_eq!(indices, (0..crds.len()).collect::<Vec<_>>());
        for (shard_index, shard) in self.shards.iter().enumerate() {
            for (&index, &hash) in shard {
                assert_eq!(hash, CrdsFilter::hash_as_u64(crds[index].value.hash()));
                assert_eq!(
                    shard_index as u64,
                    hash.checked_shr(64 - self.shard_bits).unwrap_or(0)
                );
            }
        }
    }
}

// Wrapper for 3 types of iterators we get when comparing shard_bits and
// mask_bits in find method. This is to avoid Box<dyn Iterator<Item =...>>
// which involves dynamic dispatch and is relatively slow.
/// 之所以分三种，是因为find函数那里map出来的三种的类型不一样，又不想用 Box dyn，所以用三个范型静态分发
enum Iter<R, S, T> {
    Less(R),
    Equal(S),
    Greater(T),
}

impl<R, S, T> Iterator for Iter<R, S, T>
where
    R: Iterator<Item = usize>,
    S: Iterator<Item = usize>,
    T: Iterator<Item = usize>,
{
    type Item = usize;

    /// 直接调用 next，毕竟只为了静态分发才搞得枚举
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Greater(iter) => iter.next(),
            Self::Less(iter) => iter.next(),
            Self::Equal(iter) => iter.next(),
        }
    }
}

#[cfg(test)]
mod test {
    use {
        super::*,
        crate::{
            crds::{Crds, GossipRoute},
            crds_value::CrdsValue,
        },
        rand::{thread_rng, Rng},
        solana_sdk::timing::timestamp,
        std::{collections::HashSet, iter::repeat_with, ops::Index},
    };

    fn new_test_crds_value<R: Rng>(rng: &mut R) -> VersionedCrdsValue {
        let value = CrdsValue::new_rand(rng, None);
        let label = value.label();
        let mut crds = Crds::default();
        crds.insert(value, timestamp(), GossipRoute::LocalMessage)
            .unwrap();
        crds.get::<&VersionedCrdsValue>(&label).cloned().unwrap()
    }

    // Returns true if the first mask_bits most significant bits of hash is the
    // same as the given bit mask.
    fn check_mask(value: &VersionedCrdsValue, mask: u64, mask_bits: u32) -> bool {
        let hash = CrdsFilter::hash_as_u64(value.value.hash());
        let ones = (!0u64).checked_shr(mask_bits).unwrap_or(0u64);
        (hash | ones) == (mask | ones)
    }

    // Manual filtering by scanning all the values.
    fn filter_crds_values(
        values: &[VersionedCrdsValue],
        mask: u64,
        mask_bits: u32,
    ) -> HashSet<usize> {
        values
            .iter()
            .enumerate()
            .filter_map(|(index, value)| {
                if check_mask(value, mask, mask_bits) {
                    Some(index)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn test_crds_shards_round_trip() {
        let mut rng = thread_rng();
        // Generate some random hash and crds value labels.
        let mut values: Vec<_> = repeat_with(|| new_test_crds_value(&mut rng))
            .take(4096)
            .collect();
        // Insert everything into the crds shards.
        let mut shards = CrdsShards::new(5);
        for (index, value) in values.iter().enumerate() {
            assert!(shards.insert(index, value));
        }
        shards.check(&values);
        // Remove some of the values.
        for _ in 0..512 {
            let index = rng.gen_range(0..values.len());
            let value = values.swap_remove(index);
            assert!(shards.remove(index, &value));
            if index < values.len() {
                let value = values.index(index);
                assert!(shards.remove(values.len(), value));
                assert!(shards.insert(index, value));
            }
            shards.check(&values);
        }
        // Random masks.
        for _ in 0..10 {
            let mask = rng.gen();
            for mask_bits in 0..12 {
                let mut set = filter_crds_values(&values, mask, mask_bits);
                for index in shards.find(mask, mask_bits) {
                    assert!(set.remove(&index));
                }
                assert!(set.is_empty());
            }
        }
        // Existing hash values.
        for (index, value) in values.iter().enumerate() {
            let mask = CrdsFilter::hash_as_u64(value.value.hash());
            let hits: Vec<_> = shards.find(mask, 64).collect();
            assert_eq!(hits, vec![index]);
        }
        // Remove everything.
        while !values.is_empty() {
            let index = rng.gen_range(0..values.len());
            let value = values.swap_remove(index);
            assert!(shards.remove(index, &value));
            if index < values.len() {
                let value = values.index(index);
                assert!(shards.remove(values.len(), value));
                assert!(shards.insert(index, value));
            }
            if index % 5 == 0 {
                shards.check(&values);
            }
        }
    }
}
