use {
    crate::client_error,
    serde::{Deserialize, Deserializer, Serialize, Serializer},
    solana_account_decoder_client_types::{token::UiTokenAmount, UiAccount},
    solana_clock::{Epoch, Slot, UnixTimestamp},
    solana_fee_calculator::{FeeCalculator, FeeRateGovernor},
    solana_inflation::Inflation,
    solana_transaction_error::{TransactionError, TransactionResult as Result},
    solana_transaction_status_client_types::{
        ConfirmedTransactionStatusWithSignature, TransactionConfirmationStatus, UiConfirmedBlock,
        UiInnerInstructions, UiTransactionReturnData,
    },
    std::{collections::HashMap, fmt, net::SocketAddr, str::FromStr},
    thiserror::Error,
};

/// Wrapper for rpc return types of methods that provide responses both with and without context.
/// Main purpose of this is to fix methods that lack context information in their return type,
/// without breaking backwards compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OptionalContext<T> {
    Context(Response<T>),
    NoContext(T),
}

impl<T> OptionalContext<T> {
    pub fn parse_value(self) -> T {
        match self {
            Self::Context(response) => response.value,
            Self::NoContext(value) => value,
        }
    }
}

pub type RpcResult<T> = client_error::Result<Response<T>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcResponseContext {
    pub slot: Slot,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_version: Option<RpcApiVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcApiVersion(semver::Version);

impl std::ops::Deref for RpcApiVersion {
    type Target = semver::Version;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for RpcApiVersion {
    fn default() -> Self {
        Self(solana_version::Version::default().as_semver_version())
    }
}

impl Serialize for RpcApiVersion {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for RpcApiVersion {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        Ok(RpcApiVersion(
            semver::Version::from_str(&s).map_err(serde::de::Error::custom)?,
        ))
    }
}

impl RpcResponseContext {
    pub fn new(slot: Slot) -> Self {
        Self {
            slot,
            api_version: Some(RpcApiVersion::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response<T> {
    pub context: RpcResponseContext,
    pub value: T,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RpcBlockCommitment<T> {
    pub commitment: Option<T>,
    pub total_stake: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcBlockhashFeeCalculator {
    pub blockhash: String,
    pub fee_calculator: FeeCalculator,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcBlockhash {
    pub blockhash: String,
    pub last_valid_block_height: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcFeeCalculator {
    pub fee_calculator: FeeCalculator,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcFeeRateGovernor {
    pub fee_rate_governor: FeeRateGovernor,
}

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcInflationGovernor {
    pub initial: f64,
    pub terminal: f64,
    pub taper: f64,
    pub foundation: f64,
    pub foundation_term: f64,
}

impl From<Inflation> for RpcInflationGovernor {
    fn from(inflation: Inflation) -> Self {
        Self {
            initial: inflation.initial,
            terminal: inflation.terminal,
            taper: inflation.taper,
            foundation: inflation.foundation,
            foundation_term: inflation.foundation_term,
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RpcInflationRate {
    pub total: f64,
    pub validator: f64,
    pub foundation: f64,
    pub epoch: Epoch,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcKeyedAccount {
    pub pubkey: String,
    pub account: UiAccount,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotInfo {
    pub slot: Slot,
    pub parent: Slot,
    pub root: Slot,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SlotTransactionStats {
    pub num_transaction_entries: u64,
    pub num_successful_transactions: u64,
    pub num_failed_transactions: u64,
    pub max_transactions_per_entry: u64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SlotUpdate {
    FirstShredReceived {
        slot: Slot,
        timestamp: u64,
    },
    Completed {
        slot: Slot,
        timestamp: u64,
    },
    CreatedBank {
        slot: Slot,
        parent: Slot,
        timestamp: u64,
    },
    Frozen {
        slot: Slot,
        timestamp: u64,
        stats: SlotTransactionStats,
    },
    Dead {
        slot: Slot,
        timestamp: u64,
        err: String,
    },
    OptimisticConfirmation {
        slot: Slot,
        timestamp: u64,
    },
    Root {
        slot: Slot,
        timestamp: u64,
    },
}

impl SlotUpdate {
    pub fn slot(&self) -> Slot {
        match self {
            Self::FirstShredReceived { slot, .. } => *slot,
            Self::Completed { slot, .. } => *slot,
            Self::CreatedBank { slot, .. } => *slot,
            Self::Frozen { slot, .. } => *slot,
            Self::Dead { slot, .. } => *slot,
            Self::OptimisticConfirmation { slot, .. } => *slot,
            Self::Root { slot, .. } => *slot,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", untagged)]
pub enum RpcSignatureResult {
    ProcessedSignature(ProcessedSignatureResult),
    ReceivedSignature(ReceivedSignatureResult),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcLogsResponse {
    pub signature: String, // Signature as base58 string
    pub err: Option<TransactionError>,
    pub logs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessedSignatureResult {
    pub err: Option<TransactionError>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReceivedSignatureResult {
    ReceivedSignature,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcContactInfo {
    /// Pubkey of the node as a base-58 string
    pub pubkey: String,
    /// Gossip port
    pub gossip: Option<SocketAddr>,
    /// Tvu UDP port
    pub tvu: Option<SocketAddr>,
    /// Tpu UDP port
    pub tpu: Option<SocketAddr>,
    /// Tpu QUIC port
    pub tpu_quic: Option<SocketAddr>,
    /// Tpu UDP forwards port
    pub tpu_forwards: Option<SocketAddr>,
    /// Tpu QUIC forwards port
    pub tpu_forwards_quic: Option<SocketAddr>,
    /// Tpu UDP vote port
    pub tpu_vote: Option<SocketAddr>,
    /// Server repair UDP port
    pub serve_repair: Option<SocketAddr>,
    /// JSON RPC port
    pub rpc: Option<SocketAddr>,
    /// WebSocket PubSub port
    pub pubsub: Option<SocketAddr>,
    /// Software version
    pub version: Option<String>,
    /// First 4 bytes of the FeatureSet identifier
    pub feature_set: Option<u32>,
    /// Shred version
    pub shred_version: Option<u16>,
}

/// Map of leader base58 identity pubkeys to the slot indices relative to the first epoch slot
/// 节点公钥到当前纪元中自己当 leader 的时隙索引的映射表
pub type RpcLeaderSchedule = HashMap<String, Vec<usize>>;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcBlockProductionRange {
    pub first_slot: Slot,
    pub last_slot: Slot,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcBlockProduction {
    /// Map of leader base58 identity pubkeys to a tuple of `(number of leader slots, number of blocks produced)`
    pub by_identity: HashMap<String, (usize, usize)>,
    pub range: RpcBlockProductionRange,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct RpcVersionInfo {
    /// The current version of solana-core
    pub solana_core: String,
    /// first 4 bytes of the FeatureSet identifier
    pub feature_set: Option<u32>,
}

impl fmt::Debug for RpcVersionInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.solana_core)
    }
}

impl fmt::Display for RpcVersionInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(version) = self.solana_core.split_whitespace().next() {
            // Display just the semver if possible
            write!(f, "{version}")
        } else {
            write!(f, "{}", self.solana_core)
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct RpcIdentity {
    /// The current node identity pubkey
    pub identity: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcVote {
    /// Vote account address, as base-58 encoded string
    pub vote_pubkey: String,
    pub slots: Vec<Slot>,
    pub hash: String,
    pub timestamp: Option<UnixTimestamp>,
    pub signature: String,
}

/// 投票账号状态
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcVoteAccountStatus {
    /// 当前的
    pub current: Vec<RpcVoteAccountInfo>,
    /// 有过失的
    pub delinquent: Vec<RpcVoteAccountInfo>,
}


/// 在 Solana 中，VoteAccount 是一种特殊的账户类型，用于记录验证者在网络中的投票行为和参与共识的情况。它是 Solana 网络中的一个核心组成部分，帮助确保网络的去中心化和验证者的责任性。
/// VoteAccount的作用
/// 记录验证者投票：
/// Solana 网络采用 Proof of History (PoH) 和 Proof of Stake (PoS) 的混合共识机制。在这种机制下，验证者不仅负责验证区块，还参与投票以帮助网络确定哪个区块在区块链中是合法的（即最终的）。
/// VoteAccount 用于记录这些投票信息，包括验证者对区块的选择与是否支持某些区块（即验证者投票的区块）。
/// 保证验证者活跃度：
/// 验证者通过 VoteAccount 向网络证明他们有参与共识机制，且在网络中保持活跃。每个验证者必须定期提交投票，以便参与共识并获得奖励。
/// 投票频率和质量（即是否投票支持正确的区块）直接影响验证者的奖励和声誉。如果验证者的投票记录不佳或参与度不足，他们可能会失去获得奖励的资格。
/// 奖励分配：
/// Solana 网络的奖励分配是基于验证者的参与度和投票记录来计算的。通过 VoteAccount，验证者可以获得来自网络的奖励，奖励的分配依据他们的投票情况、投票的及时性以及正确性。
/// VoteAccount的结构
/// 一个 VoteAccount 包含以下信息：
/// 验证者的身份：即验证者的身份信息。
/// 投票历史：记录验证者投票的区块，包括支持的区块和投票的时间。
/// 投票权重：根据验证者在网络中质押的资金量和投票行为计算出来的投票权重。质押越多、投票越积极，投票权重越大。
/// VoteAccount的重要性
/// 网络安全：通过记录每个验证者的投票情况，VoteAccount 使得网络可以对参与共识的验证者进行评估，保证验证者的行为透明、可审计，从而增加网络的安全性。
/// 奖励机制：VoteAccount 是验证者获得奖励的基础。积极投票、参与共识的验证者可以通过 VoteAccount 获得更多的奖励，而不活跃或表现差的验证者会失去奖励或被淘汰。
/// 验证者的去中心化与公平性：通过 VoteAccount，网络确保了验证者的去中心化和公平性。每个验证者都有机会投票，并通过他们的投票行为影响区块的最终排序。
/// 总结
/// VoteAccount 是 Solana 中用于跟踪和记录验证者投票行为的账户，它不仅对网络的共识过程至关重要，还与验证者的奖励和参与度密切相关。通过这个机制，Solana 可以鼓励验证者积极参与共识并确保区块链的去中心化和安全性。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcVoteAccountInfo {
    /// Vote account address, as base-58 encoded string
    /// 投票账号的公钥地址
    pub vote_pubkey: String,

    /// The validator identity, as base-58 encoded string
    /// 对应节点的公钥地址
    pub node_pubkey: String,

    /// The current stake, in lamports, delegated to this vote account
    /// 这个投票账号里的质押 lamports 数
    pub activated_stake: u64,

    /// An 8-bit integer used as a fraction (commission/MAX_U8) for rewards payout
    /// 佣金小数
    pub commission: u8,

    /// Whether this account is staked for the current epoch
    /// 该账号是否质押给了当前纪元
    pub epoch_vote_account: bool,

    /// Latest history of earned credits for up to `MAX_RPC_VOTE_ACCOUNT_INFO_EPOCH_CREDITS_HISTORY` epochs
    ///   each tuple is (Epoch, credits, prev_credits)
    /// 历史纪元积分，最多保存 MAX_RPC_VOTE_ACCOUNT_INFO_EPOCH_CREDITS_HISTORY 个
    pub epoch_credits: Vec<(Epoch, u64, u64)>,

    /// Most recent slot voted on by this vote account (0 if no votes exist)
    /// 最近投票的时隙
    pub last_vote: u64,

    /// Current root slot for this vote account (0 if no root slot exists)
    /// 该账号的当前根时隙
    pub root_slot: Slot,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcSignatureConfirmation {
    pub confirmations: usize,
    pub status: Result<()>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcSimulateTransactionResult {
    pub err: Option<TransactionError>,
    pub logs: Option<Vec<String>>,
    pub accounts: Option<Vec<Option<UiAccount>>>,
    pub units_consumed: Option<u64>,
    pub return_data: Option<UiTransactionReturnData>,
    pub inner_instructions: Option<Vec<UiInnerInstructions>>,
    pub replacement_blockhash: Option<RpcBlockhash>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcStorageTurn {
    pub blockhash: String,
    pub slot: Slot,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcAccountBalance {
    pub address: String,
    pub lamports: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcSupply {
    pub total: u64,
    pub circulating: u64,
    pub non_circulating: u64,
    pub non_circulating_accounts: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StakeActivationState {
    Activating,
    Active,
    Deactivating,
    Inactive,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RpcTokenAccountBalance {
    pub address: String,
    #[serde(flatten)]
    pub amount: UiTokenAmount,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcConfirmedTransactionStatusWithSignature {
    pub signature: String,
    pub slot: Slot,
    pub err: Option<TransactionError>,
    pub memo: Option<String>,
    pub block_time: Option<UnixTimestamp>,
    pub confirmation_status: Option<TransactionConfirmationStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcPerfSample {
    pub slot: Slot,
    pub num_transactions: u64,
    pub num_non_vote_transactions: Option<u64>,
    pub num_slots: u64,
    pub sample_period_secs: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcInflationReward {
    pub epoch: Epoch,
    pub effective_slot: Slot,
    pub amount: u64,            // lamports
    pub post_balance: u64,      // lamports
    pub commission: Option<u8>, // Vote account commission when the reward was credited
}

#[derive(Clone, Deserialize, Serialize, Debug, Error, Eq, PartialEq)]
pub enum RpcBlockUpdateError {
    #[error("block store error")]
    BlockStoreError,

    #[error("unsupported transaction version ({0})")]
    UnsupportedTransactionVersion(u8),
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RpcBlockUpdate {
    pub slot: Slot,
    pub block: Option<UiConfirmedBlock>,
    pub err: Option<RpcBlockUpdateError>,
}

impl From<ConfirmedTransactionStatusWithSignature> for RpcConfirmedTransactionStatusWithSignature {
    fn from(value: ConfirmedTransactionStatusWithSignature) -> Self {
        let ConfirmedTransactionStatusWithSignature {
            signature,
            slot,
            err,
            memo,
            block_time,
        } = value;
        Self {
            signature: signature.to_string(),
            slot,
            err,
            memo,
            block_time,
            confirmation_status: None,
        }
    }
}

/// 快照的时隙号
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RpcSnapshotSlotInfo {
    /// 全时隙
    pub full: Slot,
    /// 增量时隙
    pub incremental: Option<Slot>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcPrioritizationFee {
    pub slot: Slot,
    pub prioritization_fee: u64,
}

#[cfg(test)]
pub mod tests {

    use {super::*, serde_json::json};

    // Make sure that `RpcPerfSample` can read previous version JSON, one without the
    // `num_non_vote_transactions` field.
    #[test]
    fn rpc_perf_sample_deserialize_old() {
        let slot = 424;
        let num_transactions = 2597;
        let num_slots = 2783;
        let sample_period_secs = 398;

        let input = json!({
            "slot": slot,
            "numTransactions": num_transactions,
            "numSlots": num_slots,
            "samplePeriodSecs": sample_period_secs,
        })
        .to_string();

        let actual: RpcPerfSample =
            serde_json::from_str(&input).expect("Can parse RpcPerfSample from string as JSON");
        let expected = RpcPerfSample {
            slot,
            num_transactions,
            num_non_vote_transactions: None,
            num_slots,
            sample_period_secs,
        };

        assert_eq!(actual, expected);
    }

    // Make sure that `RpcPerfSample` serializes into the new `num_non_vote_transactions` field.
    #[test]
    fn rpc_perf_sample_serializes_num_non_vote_transactions() {
        let slot = 1286;
        let num_transactions = 1732;
        let num_non_vote_transactions = Some(757);
        let num_slots = 393;
        let sample_period_secs = 197;

        let input = RpcPerfSample {
            slot,
            num_transactions,
            num_non_vote_transactions,
            num_slots,
            sample_period_secs,
        };
        let actual =
            serde_json::to_value(input).expect("Can convert RpcPerfSample into a JSON value");
        let expected = json!({
            "slot": slot,
            "numTransactions": num_transactions,
            "numNonVoteTransactions": num_non_vote_transactions,
            "numSlots": num_slots,
            "samplePeriodSecs": sample_period_secs,
        });

        assert_eq!(actual, expected);
    }
}
