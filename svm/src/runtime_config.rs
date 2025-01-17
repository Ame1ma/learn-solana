use solana_compute_budget::compute_budget::ComputeBudget;

#[cfg(feature = "frozen-abi")]
impl ::solana_frozen_abi::abi_example::AbiExample for RuntimeConfig {
    fn example() -> Self {
        // RuntimeConfig is not Serialize so just rely on Default.
        RuntimeConfig::default()
    }
}

/// Encapsulates flags that can be used to tweak the runtime behavior.
/// 用于配置 sealevel 运行时行为
#[derive(Debug, Default, Clone)]
pub struct RuntimeConfig {
    /// 设置每个交易的计算资源预算
    pub compute_budget: Option<ComputeBudget>,
    /// 限制交易日志的最大字节数
    pub log_messages_bytes_limit: Option<usize>,
    /// 限制每个交易中可以操作的账户数量
    pub transaction_account_lock_limit: Option<usize>,
}
