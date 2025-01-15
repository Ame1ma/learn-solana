//! Information about the current epoch.
//!
//! As returned by the [`getEpochInfo`] RPC method.
//!
//! [`getEpochInfo`]: https://solana.com/docs/rpc/http/getepochinfo


/// 纪元信息
/// 在 Solana 中，slot 和 块高度 都与区块链的结构和时间相关，但它们代表不同的概念。
/// Slot
/// Slot 是 Solana 网络的时间单位，也可以理解为一个时间段。在 Solana 网络中，区块的生成是基于时序的，每一个时间段（slot）都有一个与之对应的区块。
/// Solana 网络使用的是 Proof of History (PoH) 共识机制，PoH 为每个 slot 生成一个唯一的时间戳，确保了区块的顺序。
/// 每个 slot 大约持续 400 毫秒，并且每个 slot 会生成一个区块（如果没有发生冲突）。
/// 块高度（Block Height）
/// 块高度 是指某个区块在区块链中的位置或顺序编号，通常是从创世区块开始的递增数字。
/// Solana 的块高度与 slot 紧密相关，但不是一一对应。因为 Solana 网络可能在某些时刻并行生成多个区块（例如，若存在不同的验证者），每个区块会有一个唯一的块高度。
/// Slot 和 块高度的关系
/// 在 Solana 中，块高度和 slot 是有一定的对应关系的。每个 slot 会生成一个区块，区块的高度通常等于这个 slot 的编号（如果没有并行或分叉的情况）。
/// 块高度 ≈ slot 数，但并不是完全一一对应，因为在并行生产区块或者分叉时，某些块可能会被丢弃（例如，最长链法则下会丢弃较短链的区块）。
/// 总结来说，slot 是 Solana 区块链的时间单位，而 块高度 是区块的顺序编号。每个 slot 通常会有一个区块，并且其块高度大致等于其 slot 的编号，但在分叉的情况下，可能会出现不同的块高度与 slot 不完全对应的情况。
#[cfg_attr(
    feature = "serde",
    derive(serde_derive::Deserialize, serde_derive::Serialize),
    serde(rename_all = "camelCase")
)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpochInfo {
    /// The current epoch
    /// 当前纪元号
    pub epoch: u64,

    /// The current slot, relative to the start of the current epoch
    /// 当前时隙到了该纪元的第几个
    pub slot_index: u64,

    /// The number of slots in this epoch
    /// 纪元总共多少个时隙
    pub slots_in_epoch: u64,

    /// The absolute current slot
    /// 当前时隙号
    pub absolute_slot: u64,

    /// The current block height
    /// 当前块高
    pub block_height: u64,

    /// Total number of transactions processed without error since genesis
    /// 创世区块以来的交易数
    pub transaction_count: Option<u64>,
}
