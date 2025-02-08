use {
    crate::{
        crds_data::{self, MAX_SLOT, MAX_WALLCLOCK},
        protocol::MAX_CRDS_OBJECT_SIZE,
    },
    bincode::serialized_size,
    bv::BitVec,
    flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress},
    solana_sanitize::{Sanitize, SanitizeError},
    solana_sdk::{clock::Slot, pubkey::Pubkey},
};

/// 每项最多的时隙数
pub const MAX_SLOTS_PER_ENTRY: usize = 2048 * 8;
/// 非压缩时隙列表
#[cfg_attr(feature = "frozen-abi", derive(AbiExample))]
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Uncompressed {
    /// 第一个时隙
    pub first_slot: Slot,
    /// 时隙个数
    pub num: usize,
    /// 时隙列表，紧密的放在里面，每8个字节是一个时隙
    pub slots: BitVec<u8>,
}

impl Sanitize for Uncompressed {
    /// 时隙列表格式需正确
    fn sanitize(&self) -> std::result::Result<(), SanitizeError> {
        // 第一个时隙不能太大
        if self.first_slot >= MAX_SLOT {
            return Err(SanitizeError::ValueOutOfBounds);
        }
        // 时隙个数不能太多
        if self.num >= MAX_SLOTS_PER_ENTRY {
            return Err(SanitizeError::ValueOutOfBounds);
        }
        // 时隙列表需要被8整除
        if self.slots.len() % 8 != 0 {
            // Uncompressed::new() ensures the length is always a multiple of 8
            return Err(SanitizeError::ValueOutOfBounds);
        }
        // 没有冗余容量
        if self.slots.len() != self.slots.capacity() {
            // A BitVec<u8> with a length that's a multiple of 8 will always have len() equal to
            // capacity(), assuming no bit manipulation
            return Err(SanitizeError::ValueOutOfBounds);
        }
        Ok(())
    }
}

/// Flate2 是 DEFLATE 算法的一个实现，它通常用于对数据进行压缩和解压缩。
/// DEFLATE 是一种无损压缩算法，广泛用于许多流行的文件格式，如 ZIP 和 GZIP。
/// Flate2 是 Rust 编程语言中的一个库，提供了 DEFLATE 算法的压缩和解压缩功能。
#[cfg_attr(feature = "frozen-abi", derive(AbiExample))]
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Flate2 {
    /// 第一个时隙
    pub first_slot: Slot,
    /// 数量
    pub num: usize,
    /// 压缩后时隙列表
    #[serde(with = "serde_bytes")]
    pub compressed: Vec<u8>,
}

impl Sanitize for Flate2 {
    fn sanitize(&self) -> std::result::Result<(), SanitizeError> {
        // 第一个时隙不能太大
        if self.first_slot >= MAX_SLOT {
            return Err(SanitizeError::ValueOutOfBounds);
        }
        // 时隙个数不能太多
        if self.num >= MAX_SLOTS_PER_ENTRY {
            return Err(SanitizeError::ValueOutOfBounds);
        }
        Ok(())
    }
}

/// 纪元时隙错误
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    CompressError,
    DecompressError,
}

pub type Result<T> = std::result::Result<T, Error>;

impl std::convert::From<flate2::CompressError> for Error {
    fn from(_e: flate2::CompressError) -> Error {
        Error::CompressError
    }
}
impl std::convert::From<flate2::DecompressError> for Error {
    fn from(_e: flate2::DecompressError) -> Error {
        Error::DecompressError
    }
}

impl Flate2 {
    /// 压缩
    fn deflate(mut unc: Uncompressed) -> Result<Self> {
        let mut compressed = Vec::with_capacity(unc.slots.block_capacity());
        let mut compressor = Compress::new(Compression::best(), false);
        let first_slot = unc.first_slot;
        let num = unc.num;
        unc.slots.shrink_to_fit();
        let bits = unc.slots.into_boxed_slice();
        compressor.compress_vec(&bits, &mut compressed, FlushCompress::Finish)?;
        let rv = Self {
            first_slot,
            num,
            compressed,
        };
        let _ = rv.inflate()?;
        Ok(rv)
    }
    /// 解压
    pub fn inflate(&self) -> Result<Uncompressed> {
        //add some head room for the decompressor which might spill more bits
        let mut uncompressed = Vec::with_capacity(32 + (self.num + 4) / 8);
        let mut decompress = Decompress::new(false);
        decompress.decompress_vec(&self.compressed, &mut uncompressed, FlushDecompress::Finish)?;
        Ok(Uncompressed {
            first_slot: self.first_slot,
            num: self.num,
            slots: BitVec::from_bits(&uncompressed),
        })
    }
}

impl Uncompressed {
    /// 新建时隙列表
    pub fn new(max_size: usize) -> Self {
        Self {
            num: 0,
            first_slot: 0,
            slots: BitVec::new_fill(false, 8 * max_size as u64),
        }
    }
    /// 取出时隙列表
    pub fn to_slots(&self, min_slot: Slot) -> Vec<Slot> {
        let mut rv = vec![];
        let start = if min_slot < self.first_slot {
            0
        } else {
            /// 不从第一个开始
            (min_slot - self.first_slot) as usize
        };
        /// 从 slots 里面依次取时隙
        for i in start..self.num {
            if i >= self.slots.len() as usize {
                break;
            }
            if self.slots.get(i as u64) {
                rv.push(self.first_slot + i as Slot);
            }
        }
        rv
    }
    /// 添加时隙，返回添加了多少个
    pub fn add(&mut self, slots: &[Slot]) -> usize {
        for (i, s) in slots.iter().enumerate() {
            /// 设置第一个时隙
            if self.num == 0 {
                self.first_slot = *s;
            }
            /// 达到了每项最多的时隙数，结束
            if self.num >= MAX_SLOTS_PER_ENTRY {
                return i;
            }
            /// 太早了
            if *s < self.first_slot {
                return i;
            }
            /// 超长了
            if *s - self.first_slot >= self.slots.len() {
                return i;
            }
            self.slots.set(*s - self.first_slot, true);
            self.num = std::cmp::max(self.num, 1 + (*s - self.first_slot) as usize);
        }
        slots.len()
    }
}

/// 压缩的时隙
#[cfg_attr(feature = "frozen-abi", derive(AbiExample, AbiEnumVisitor))]
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum CompressedSlots {
    /// 压缩的
    Flate2(Flate2),
    /// 不压缩的
    Uncompressed(Uncompressed),
}

impl Sanitize for CompressedSlots {
    /// 时隙列表格式需正确
    fn sanitize(&self) -> std::result::Result<(), SanitizeError> {
        match self {
            CompressedSlots::Uncompressed(a) => a.sanitize(),
            CompressedSlots::Flate2(b) => b.sanitize(),
        }
    }
}

impl Default for CompressedSlots {
    fn default() -> Self {
        CompressedSlots::new(0)
    }
}

/// 都是分发到各自的类型，压缩和不压缩
impl CompressedSlots {
    /// 新建
    pub(crate) fn new(max_size: usize) -> Self {
        CompressedSlots::Uncompressed(Uncompressed::new(max_size))
    }

    /// 第一个时隙
    pub fn first_slot(&self) -> Slot {
        match self {
            CompressedSlots::Uncompressed(a) => a.first_slot,
            CompressedSlots::Flate2(b) => b.first_slot,
        }
    }

    /// 时隙数
    pub fn num_slots(&self) -> usize {
        match self {
            CompressedSlots::Uncompressed(a) => a.num,
            CompressedSlots::Flate2(b) => b.num,
        }
    }

    /// 添加
    pub fn add(&mut self, slots: &[Slot]) -> usize {
        match self {
            CompressedSlots::Uncompressed(vals) => vals.add(slots),
            CompressedSlots::Flate2(_) => 0,
        }
    }
    /// 获取时隙列表
    pub fn to_slots(&self, min_slot: Slot) -> Result<Vec<Slot>> {
        match self {
            CompressedSlots::Uncompressed(vals) => Ok(vals.to_slots(min_slot)),
            CompressedSlots::Flate2(vals) => {
                let unc = vals.inflate()?;
                Ok(unc.to_slots(min_slot))
            }
        }
    }
    /// 压缩，真空袋抽气，刚好从Uncompressed转换为Flate2
    pub fn deflate(&mut self) -> Result<()> {
        match self {
            CompressedSlots::Uncompressed(vals) => {
                let unc = vals.clone();
                let compressed = Flate2::deflate(unc)?;
                *self = CompressedSlots::Flate2(compressed);
                Ok(())
            }
            CompressedSlots::Flate2(_) => Ok(()),
        }
    }
}

/// 纪元时隙
#[cfg_attr(feature = "frozen-abi", derive(AbiExample))]
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct EpochSlots {
    /// 来源
    pub from: Pubkey,
    /// 时隙列表
    pub slots: Vec<CompressedSlots>,
    /// 创建时的本地现实时间
    pub wallclock: u64,
}

impl Sanitize for EpochSlots {
    fn sanitize(&self) -> std::result::Result<(), SanitizeError> {
        /// 现实时间不能太大
        if self.wallclock >= MAX_WALLCLOCK {
            return Err(SanitizeError::ValueOutOfBounds);
        }
        self.from.sanitize()?;
        /// 时隙消毒
        self.slots.sanitize()
    }
}

use std::fmt;
impl fmt::Debug for EpochSlots {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let num_slots: usize = self.slots.iter().map(|s| s.num_slots()).sum();
        let lowest_slot = self
            .slots
            .iter()
            .map(|s| s.first_slot())
            .fold(0, std::cmp::min);
        write!(
            f,
            "EpochSlots {{ from: {} num_slots: {} lowest_slot: {} wallclock: {} }}",
            self.from, num_slots, lowest_slot, self.wallclock
        )
    }
}

impl EpochSlots {
    /// 新建纪元时隙
    pub fn new(from: Pubkey, now: u64) -> Self {
        Self {
            from,
            wallclock: now,
            slots: vec![],
        }
    }
    /// 填满
    pub fn fill(&mut self, slots: &[Slot], now: u64) -> usize {
        let mut num = 0;
        self.wallclock = std::cmp::max(now, self.wallclock + 1);
        while num < slots.len() {
            num += self.add(&slots[num..]);
            if num < slots.len() {
                if self.deflate().is_err() {
                    return num;
                }
                let space = self.max_compressed_slot_size();
                if space > 0 {
                    let cslot = CompressedSlots::new(space as usize);
                    self.slots.push(cslot);
                } else {
                    return num;
                }
            }
        }
        num
    }
    /// 增加时隙
    pub fn add(&mut self, slots: &[Slot]) -> usize {
        let mut num = 0;
        for s in &mut self.slots {
            num += s.add(&slots[num..]);
            if num >= slots.len() {
                break;
            }
        }
        num
    }
    /// 压缩
    pub fn deflate(&mut self) -> Result<()> {
        for s in self.slots.iter_mut() {
            s.deflate()?;
        }
        Ok(())
    }
    /// 最大压缩后时隙大小
    pub fn max_compressed_slot_size(&self) -> isize {
        let len_header = serialized_size(self).unwrap();
        let len_slot = serialized_size(&CompressedSlots::default()).unwrap();
        MAX_CRDS_OBJECT_SIZE as isize - (len_header + len_slot) as isize
    }

    /// 第一个时隙
    pub fn first_slot(&self) -> Option<Slot> {
        self.slots.iter().map(|s| s.first_slot()).min()
    }

    /// 获取时隙列表
    pub fn to_slots(&self, min_slot: Slot) -> Vec<Slot> {
        self.slots
            .iter()
            .filter(|s| min_slot < s.first_slot() + s.num_slots() as u64)
            .filter_map(|s| s.to_slots(min_slot).ok())
            .flatten()
            .collect()
    }

    /// New random EpochSlots for tests and simulations.
    pub(crate) fn new_rand<R: rand::Rng>(rng: &mut R, pubkey: Option<Pubkey>) -> Self {
        let now = crds_data::new_rand_timestamp(rng);
        let pubkey = pubkey.unwrap_or_else(solana_sdk::pubkey::new_rand);
        let mut epoch_slots = Self::new(pubkey, now);
        let num_slots = rng.gen_range(0..20);
        let slots: Vec<_> = std::iter::repeat_with(|| 47825632 + rng.gen_range(0..512))
            .take(num_slots)
            .collect();
        epoch_slots.add(&slots);
        epoch_slots
    }
}

#[cfg(test)]
mod tests {
    use {super::*, rand::Rng, std::iter::repeat_with};

    #[test]
    fn test_epoch_slots_max_size() {
        let epoch_slots = EpochSlots::default();
        assert!(epoch_slots.max_compressed_slot_size() > 0);
    }

    #[test]
    fn test_epoch_slots_uncompressed_add_1() {
        let mut slots = Uncompressed::new(1);
        assert_eq!(slots.slots.capacity(), 8);
        assert_eq!(slots.add(&[1]), 1);
        assert_eq!(slots.to_slots(1), vec![1]);
        assert!(slots.to_slots(2).is_empty());
    }

    #[test]
    fn test_epoch_slots_to_slots_overflow() {
        let mut slots = Uncompressed::new(1);
        slots.num = 100;
        assert!(slots.to_slots(0).is_empty());
    }

    #[test]
    fn test_epoch_slots_uncompressed_add_2() {
        let mut slots = Uncompressed::new(1);
        assert_eq!(slots.add(&[1, 2]), 2);
        assert_eq!(slots.to_slots(1), vec![1, 2]);
    }
    #[test]
    fn test_epoch_slots_uncompressed_add_3a() {
        let mut slots = Uncompressed::new(1);
        assert_eq!(slots.add(&[1, 3, 2]), 3);
        assert_eq!(slots.to_slots(1), vec![1, 2, 3]);
    }

    #[test]
    fn test_epoch_slots_uncompressed_add_3b() {
        let mut slots = Uncompressed::new(1);
        assert_eq!(slots.add(&[1, 10, 2]), 1);
        assert_eq!(slots.to_slots(1), vec![1]);
    }

    #[test]
    fn test_epoch_slots_uncompressed_add_3c() {
        let mut slots = Uncompressed::new(2);
        assert_eq!(slots.add(&[1, 10, 2]), 3);
        assert_eq!(slots.to_slots(1), vec![1, 2, 10]);
        assert_eq!(slots.to_slots(2), vec![2, 10]);
        assert_eq!(slots.to_slots(3), vec![10]);
        assert!(slots.to_slots(11).is_empty());
    }
    #[test]
    fn test_epoch_slots_compressed() {
        let mut slots = Uncompressed::new(100);
        slots.add(&[1, 701, 2]);
        assert_eq!(slots.num, 701);
        let compressed = Flate2::deflate(slots).unwrap();
        assert_eq!(compressed.first_slot, 1);
        assert_eq!(compressed.num, 701);
        assert!(compressed.compressed.len() < 32);
        let slots = compressed.inflate().unwrap();
        assert_eq!(slots.first_slot, 1);
        assert_eq!(slots.num, 701);
        assert_eq!(slots.to_slots(1), vec![1, 2, 701]);
    }

    #[test]
    fn test_epoch_slots_sanitize() {
        let mut slots = Uncompressed::new(100);
        slots.add(&[1, 701, 2]);
        assert_eq!(slots.num, 701);
        assert!(slots.sanitize().is_ok());

        let mut o = slots.clone();
        o.first_slot = MAX_SLOT;
        assert_eq!(o.sanitize(), Err(SanitizeError::ValueOutOfBounds));

        let mut o = slots.clone();
        o.num = MAX_SLOTS_PER_ENTRY;
        assert_eq!(o.sanitize(), Err(SanitizeError::ValueOutOfBounds));

        let mut o = slots.clone();
        o.slots = BitVec::new_fill(false, 7); // Length not a multiple of 8
        assert_eq!(o.sanitize(), Err(SanitizeError::ValueOutOfBounds));

        let mut o = slots.clone();
        o.slots = BitVec::with_capacity(8); // capacity() not equal to len()
        assert_eq!(o.sanitize(), Err(SanitizeError::ValueOutOfBounds));

        let compressed = Flate2::deflate(slots).unwrap();
        assert!(compressed.sanitize().is_ok());

        let mut o = compressed.clone();
        o.first_slot = MAX_SLOT;
        assert_eq!(o.sanitize(), Err(SanitizeError::ValueOutOfBounds));

        let mut o = compressed;
        o.num = MAX_SLOTS_PER_ENTRY;
        assert_eq!(o.sanitize(), Err(SanitizeError::ValueOutOfBounds));

        let mut slots = EpochSlots::default();
        let range: Vec<Slot> = (0..5000).collect();
        assert_eq!(slots.fill(&range, 1), 5000);
        assert_eq!(slots.wallclock, 1);
        assert!(slots.sanitize().is_ok());

        let mut o = slots;
        o.wallclock = MAX_WALLCLOCK;
        assert_eq!(o.sanitize(), Err(SanitizeError::ValueOutOfBounds));
    }

    #[test]
    fn test_epoch_slots_fill_range() {
        let range: Vec<Slot> = (0..5000).collect();
        let mut slots = EpochSlots::default();
        assert_eq!(slots.fill(&range, 1), 5000);
        assert_eq!(slots.wallclock, 1);
        assert_eq!(slots.to_slots(0), range);
        assert_eq!(slots.to_slots(4999), vec![4999]);
        assert!(slots.to_slots(5000).is_empty());
    }
    #[test]
    fn test_epoch_slots_fill_sparce_range() {
        let range: Vec<Slot> = (0..5000).map(|x| x * 3).collect();
        let mut slots = EpochSlots::default();
        assert_eq!(slots.fill(&range, 2), 5000);
        assert_eq!(slots.wallclock, 2);
        assert_eq!(slots.slots.len(), 3);
        assert_eq!(slots.slots[0].first_slot(), 0);
        assert_ne!(slots.slots[0].num_slots(), 0);
        let next = slots.slots[0].num_slots() as u64 + slots.slots[0].first_slot();
        assert!(slots.slots[1].first_slot() >= next);
        assert_ne!(slots.slots[1].num_slots(), 0);
        assert_ne!(slots.slots[2].num_slots(), 0);
        assert_eq!(slots.to_slots(0), range);
        assert_eq!(slots.to_slots(4999 * 3), vec![4999 * 3]);
    }

    #[test]
    fn test_epoch_slots_fill_large_sparce_range() {
        let range: Vec<Slot> = (0..5000).map(|x| x * 7).collect();
        let mut slots = EpochSlots::default();
        assert_eq!(slots.fill(&range, 2), 5000);
        assert_eq!(slots.to_slots(0), range);
    }

    fn make_rand_slots<R: Rng>(rng: &mut R) -> impl Iterator<Item = Slot> + '_ {
        repeat_with(|| rng.gen_range(1..5)).scan(0, |slot, step| {
            *slot += step;
            Some(*slot)
        })
    }

    #[test]
    fn test_epoch_slots_fill_uncompressed_random_range() {
        let mut rng = rand::thread_rng();
        for _ in 0..10 {
            let range: Vec<Slot> = make_rand_slots(&mut rng).take(5000).collect();
            let sz = EpochSlots::default().max_compressed_slot_size();
            let mut slots = Uncompressed::new(sz as usize);
            let sz = slots.add(&range);
            let slots = slots.to_slots(0);
            assert_eq!(slots.len(), sz);
            assert_eq!(slots[..], range[..sz]);
        }
    }

    #[test]
    fn test_epoch_slots_fill_compressed_random_range() {
        let mut rng = rand::thread_rng();
        for _ in 0..10 {
            let range: Vec<Slot> = make_rand_slots(&mut rng).take(5000).collect();
            let sz = EpochSlots::default().max_compressed_slot_size();
            let mut slots = Uncompressed::new(sz as usize);
            let sz = slots.add(&range);
            let mut slots = CompressedSlots::Uncompressed(slots);
            slots.deflate().unwrap();
            let slots = slots.to_slots(0).unwrap();
            assert_eq!(slots.len(), sz);
            assert_eq!(slots[..], range[..sz]);
        }
    }

    #[test]
    fn test_epoch_slots_fill_random_range() {
        let mut rng = rand::thread_rng();
        for _ in 0..10 {
            let range: Vec<Slot> = make_rand_slots(&mut rng).take(5000).collect();
            let mut slots = EpochSlots::default();
            let sz = slots.fill(&range, 1);
            let last = range[sz - 1];
            assert_eq!(
                last,
                slots.slots.last().unwrap().first_slot()
                    + slots.slots.last().unwrap().num_slots() as u64
                    - 1
            );
            for s in &slots.slots {
                assert!(s.to_slots(0).is_ok());
            }
            let slots = slots.to_slots(0);
            assert_eq!(slots[..], range[..slots.len()]);
            assert_eq!(sz, slots.len())
        }
    }
}
