use std::net::{IpAddr, SocketAddr};

/// socket的监听范围，公网或公网内网都行
#[derive(Clone, Copy)]
pub enum SocketAddrSpace {
    // 公网内网都行
    Unspecified,
    // 公网
    Global,
}

impl SocketAddrSpace {
    /// 根据是否允许监听内网，来创建，后面可以用来检查 ip 地址是不是内网
    pub fn new(allow_private_addr: bool) -> Self {
        if allow_private_addr {
            SocketAddrSpace::Unspecified
        } else {
            SocketAddrSpace::Global
        }
    }

    /// Returns true if the IP address is valid.
    /// 检查是不是
    #[inline]
    #[must_use]
    pub fn check(&self, addr: &SocketAddr) -> bool {
        if matches!(self, SocketAddrSpace::Unspecified) {
            return true;
        }
        // TODO: remove these once IpAddr::is_global is stable.
        match addr.ip() {
            IpAddr::V4(addr) => {
                // TODO: Consider excluding:
                //    addr.is_link_local() || addr.is_broadcast()
                // || addr.is_documentation() || addr.is_unspecified()
                !(addr.is_private() || addr.is_loopback())
            }
            IpAddr::V6(addr) => {
                // TODO: Consider excluding:
                // addr.is_unspecified(),
                !addr.is_loopback()
            }
        }
    }
}
