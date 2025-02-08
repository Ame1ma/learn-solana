//! A trait for sanitizing values and members of over the wire messages.
//! 对收到的消息进行消毒，只提供 trait，消毒要自己实现
//! 实现应该递归地遍历数据结构并清理所有结构成员和枚举子句。杀毒不包括签名验证检查，这些检查由另一个通道处理。消毒检查应包括但不限于：
//! 所有索引值都在范围内。
//! 所有值都在其静态最大/最小范围内。

use {core::fmt, std::error::Error};

/// 消毒错误
#[derive(PartialEq, Debug, Eq, Clone)]
pub enum SanitizeError {
    /// 索引越界
    IndexOutOfBounds,
    /// 值越界
    ValueOutOfBounds,
    /// 无效的值
    InvalidValue,
}

impl Error for SanitizeError {}

impl fmt::Display for SanitizeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            SanitizeError::IndexOutOfBounds => f.write_str("index out of bounds"),
            SanitizeError::ValueOutOfBounds => f.write_str("value out of bounds"),
            SanitizeError::InvalidValue => f.write_str("invalid value"),
        }
    }
}

/// A trait for sanitizing values and members of over-the-wire messages.
///
/// Implementation should recursively descend through the data structure and
/// sanitize all struct members and enum clauses. Sanitize excludes signature-
/// verification checks, those are handled by another pass. Sanitize checks
/// should include but are not limited to:
///
/// - All index values are in range.
/// - All values are within their static max/min bounds.
/// 实现应该递归地遍历数据结构并清理所有结构成员和枚举子句。杀毒不包括签名验证检查，这些检查由另一个通道处理。消毒检查应包括但不限于：
/// 所有索引值都在范围内。
/// 所有值都在其静态最大/最小范围内。
pub trait Sanitize {
    /// 消毒，对传入消息进行验证并清除不合理值，防止把集群炸了
    fn sanitize(&self) -> Result<(), SanitizeError> {
        Ok(())
    }
}

impl<T: Sanitize> Sanitize for Vec<T> {
    /// 给列表实现
    fn sanitize(&self) -> Result<(), SanitizeError> {
        for x in self.iter() {
            x.sanitize()?;
        }
        Ok(())
    }
}
