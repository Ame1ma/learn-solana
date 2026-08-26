use {
    lru::LruCache,
    rand::{CryptoRng, Rng},
    serde_big_array::BigArray,
    siphasher::sip::SipHasher24,
    solana_sanitize::{Sanitize, SanitizeError},
    solana_sdk::{
        hash::Hash,
        pubkey::Pubkey,
        signature::{Keypair, Signable, Signature, Signer},
    },
    std::{
        borrow::Cow,
        hash::{Hash as _, Hasher},
        net::SocketAddr,
        time::{Duration, Instant},
    },
};

//! ping pong
//! ping 生成一个 token 并签名，pong 验证签名，pong 再哈希这个 token，对哈希进行签名并响应

/// 密钥刷新周期
const KEY_REFRESH_CADENCE: Duration = Duration::from_secs(60);
/// ping pong 前缀
const PING_PONG_HASH_PREFIX: &[u8] = "SOLANA_PING_PONG".as_bytes();

// For backward compatibility we are using a const generic parameter here.
// N should always be >= 8 and only the first 8 bytes are used. So the new code
// should only use N == 8.
/// ping 结构体
#[cfg_attr(feature = "frozen-abi", derive(AbiExample))]
#[derive(Debug, Deserialize, Serialize)]
pub struct Ping<const N: usize> {
    /// 来源节点
    from: Pubkey,
    /// 令牌
    #[serde(with = "BigArray")]
    token: [u8; N],
    /// 签名
    signature: Signature,
}

/// pong 结构体
#[cfg_attr(feature = "frozen-abi", derive(AbiExample))]
#[derive(Debug, Deserialize, Serialize)]
pub struct Pong {
    /// 来源节点
    from: Pubkey,
    /// 哈希
    hash: Hash, // Hash of received ping token.
    /// 签名
    signature: Signature,
}

/// Maintains records of remote nodes which have returned a valid response to a
/// ping message, and on-the-fly ping messages pending a pong response from the
/// remote node.
/// Const generic parameter N corresponds to token size in Ping<N> type.
/// ping 缓存
pub struct PingCache<const N: usize> {
    // Time-to-live of received pong messages.
    /// 存活时间
    ttl: Duration,
    // Rate limit delay to generate pings for a given address
    /// 生成ping的速率限制延迟
    rate_limit_delay: Duration,
    // Hashers initialized with random keys, rotated at KEY_REFRESH_CADENCE.
    // Because at the moment that the keys are rotated some pings might already
    // be in the flight, we need to keep the two most recent hashers.
    /// 两个随机密钥的哈希器，在 KEY_REFRESH_CADENCE 时旋转
    /// 因为当密钥旋转时，一些ping可能已经在飞行中，我们需要保持两个最近的哈希器
    hashers: [SipHasher24; 2],
    // When hashers were last refreshed.
    /// 哈希器最后刷新时间
    key_refresh: Instant,
    // Timestamp of last ping message sent to a remote node.
    // Used to rate limit pings to remote nodes.
    /// 上次发送ping消息的时间
    pings: LruCache<(Pubkey, SocketAddr), Instant>,
    // Verified pong responses from remote nodes.
    /// 验证过的pong响应
    pongs: LruCache<(Pubkey, SocketAddr), Instant>,
}

impl<const N: usize> Ping<N> {
    /// 构造ping消息
    pub fn new(token: [u8; N], keypair: &Keypair) -> Self {
        // 使用私钥签名token
        let signature = keypair.sign_message(&token);
        Ping {
            from: keypair.pubkey(),
            token,
            signature,
        }
    }
}

impl<const N: usize> Sanitize for Ping<N> {
    /// 验证ping消息
    fn sanitize(&self) -> Result<(), SanitizeError> {
        // 验证来源节点
        self.from.sanitize()?;
        // TODO Add self.token.sanitize()?; when rust's
        // specialization feature becomes stable.
        // 验证签名
        self.signature.sanitize()
    }
}

impl<const N: usize> Signable for Ping<N> {
    #[inline]
    fn pubkey(&self) -> Pubkey {
        self.from
    }

    #[inline]
    fn signable_data(&self) -> Cow<[u8]> {
        Cow::Borrowed(&self.token)
    }

    #[inline]
    fn get_signature(&self) -> Signature {
        self.signature
    }

    fn set_signature(&mut self, signature: Signature) {
        self.signature = signature;
    }
}

impl Pong {
    pub fn new<const N: usize>(ping: &Ping<N>, keypair: &Keypair) -> Self {
        // 计算ping token的哈希
        let hash = hash_ping_token(&ping.token);
        Pong {
            from: keypair.pubkey(),
            // 用ping token的哈希
            hash,
            // 使用私钥签名哈希
            signature: keypair.sign_message(hash.as_ref()),
        }
    }

    pub fn from(&self) -> &Pubkey {
        &self.from
    }
}

impl Sanitize for Pong {
    fn sanitize(&self) -> Result<(), SanitizeError> {
        self.from.sanitize()?;
        self.hash.sanitize()?;
        self.signature.sanitize()
    }
}

impl Signable for Pong {
    fn pubkey(&self) -> Pubkey {
        self.from
    }

    fn signable_data(&self) -> Cow<[u8]> {
        Cow::Owned(self.hash.as_ref().into())
    }

    fn get_signature(&self) -> Signature {
        self.signature
    }

    fn set_signature(&mut self, signature: Signature) {
        self.signature = signature;
    }
}

impl<const N: usize> PingCache<N> {
    /// 构造ping缓存
    pub fn new<R: Rng + CryptoRng>(
        rng: &mut R,
        now: Instant,
        ttl: Duration,
        rate_limit_delay: Duration,
        cap: usize,
    ) -> Self {
        // Sanity check ttl/rate_limit_delay
        // 检查存活时间是否大于生成ping的速率限制延迟
        assert!(rate_limit_delay <= ttl / 2);
        Self {
            ttl,
            rate_limit_delay,
            hashers: std::array::from_fn(|_| SipHasher24::new_with_key(&rng.gen())),
            key_refresh: now,
            pings: LruCache::new(cap),
            pongs: LruCache::new(cap),
        }
    }

    /// Checks if the pong hash, pubkey and socket match a ping message sent
    /// out previously. If so records current timestamp for the remote node and
    /// returns true.
    /// Note: Does not verify the signature.
    /// 检查并记录pong响应
    pub fn add(&mut self, pong: &Pong, socket: SocketAddr, now: Instant) -> bool {
        // 获取远程节点
        let remote_node = (pong.pubkey(), socket);
        // 检查是否存在匹配的哈希
        if !self.hashers.iter().copied().any(|hasher| {
            // 生成ping token
            let token = make_ping_token::<N>(hasher, &remote_node);
            // 检查哈希是否匹配
            hash_ping_token(&token) == pong.hash
        }) {
            // 不匹配，返回false
            return false;
        };
        // 记录当前时间
        self.pongs.put(remote_node, now);
        // 返回true
        true
    }

    /// Checks if the remote node has been pinged recently. If not, calls the
    /// given function to generates a new ping message, records current
    /// timestamp and hash of ping token, and returns the ping message.
    /// 检查是否需要生成新的ping消息，如果需要，则生成新的ping消息，并记录当前时间戳和ping token的哈希，并返回ping消息。
    fn maybe_ping<R: Rng + CryptoRng>(
        &mut self,
        rng: &mut R,
        keypair: &Keypair,
        now: Instant,
        remote_node: (Pubkey, SocketAddr),
    ) -> Option<Ping<N>> {
        // Rate limit consecutive pings sent to a remote node.
        // 检查是否需要限制生成ping消息
        if matches!(self.pings.peek(&remote_node),
            Some(&t) if now.saturating_duration_since(t) < self.rate_limit_delay)
        {
            // 需要限制，返回None
            return None;
        }
        // 记录当前时间
        self.pings.put(remote_node, now);
        // 刷新密钥
        self.maybe_refresh_key(rng, now);
        // 生成ping token
        let token = make_ping_token::<N>(self.hashers[0], &remote_node);
        // 返回ping消息
        Some(Ping::new(token, keypair))
    }

    /// Returns true if the remote node has responded to a ping message.
    /// Removes expired pong messages. In order to extend verifications before
    /// expiration, if the pong message is not too recent, and the node has not
    /// been pinged recently, calls the given function to generates a new ping
    /// message, records current timestamp and hash of ping token, and returns
    /// the ping message.
    /// Caller should verify if the socket address is valid. (e.g. by using
    /// ContactInfo::is_valid_address).
    /// 检查节点是否收到pong，如果收到，则返回true，否则返回false，并生成新的ping消息。
    pub fn check<R: Rng + CryptoRng>(
        &mut self,
        rng: &mut R,
        keypair: &Keypair,
        now: Instant,
        remote_node: (Pubkey, SocketAddr),
    ) -> (bool, Option<Ping<N>>) {
        // 检查节点是否收到pong
        let (check, should_ping) = match self.pongs.get(&remote_node) {
            // 没有收到，返回false，并生成新的ping消息
            None => (false, true),
            // 收到，检查是否过期
            Some(t) => {
                let age = now.saturating_duration_since(*t);
                // Pop if the pong message has expired.
                // 如果pong消息过期，则删除
                if age > self.ttl {
                    self.pongs.pop(&remote_node);
                }
                // If the pong message is not too recent, generate a new ping
                // message to extend remote node verification.
                // 如果pong消息不是太久远，则生成新的ping消息
                (true, age > self.ttl / 8)
            }
        };
        // 如果需要生成新的ping消息，则生成新的ping消息
        let ping = should_ping
            .then(|| self.maybe_ping(rng, keypair, now, remote_node))
            .flatten();
        (check, ping)
    }

    /// 刷新密钥
    fn maybe_refresh_key<R: Rng + CryptoRng>(&mut self, rng: &mut R, now: Instant) {
        // 检查是否需要刷新密钥，
        if now.checked_duration_since(self.key_refresh) > Some(KEY_REFRESH_CADENCE) {
            let hasher = SipHasher24::new_with_key(&rng.gen());
            self.hashers[1] = std::mem::replace(&mut self.hashers[0], hasher);
            self.key_refresh = now;
        }
    }

    /// Only for tests and simulations.
    /// 模拟pong
    pub fn mock_pong(&mut self, node: Pubkey, socket: SocketAddr, now: Instant) {
        self.pongs.put((node, socket), now);
    }
}

/// 生成ping token
/// token是根据节点地址生成的，每个节点固定
fn make_ping_token<const N: usize>(
    mut hasher: SipHasher24,
    remote_node: &(Pubkey, SocketAddr),
) -> [u8; N] {
    // TODO: Consider including local node's (pubkey, socket-addr).
    // 计算哈希
    remote_node.hash(&mut hasher);
    // 转换为小端字节
    let hash = hasher.finish().to_le_bytes();
    // 断言哈希长度大于等于u64
    debug_assert!(N >= std::mem::size_of::<u64>());
    // 创建token
    let mut token = [0u8; N];
    // 复制hash到token
    token[..std::mem::size_of::<u64>()].copy_from_slice(&hash);
    // 返回token
    token
}

/// 哈希ping token
fn hash_ping_token<const N: usize>(token: &[u8; N]) -> Hash {
    solana_sdk::hash::hashv(&[PING_PONG_HASH_PREFIX, token])
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        std::{
            collections::HashSet,
            iter::repeat_with,
            net::{Ipv4Addr, SocketAddrV4},
        },
    };

    #[test]
    fn test_ping_pong() {
        let mut rng = rand::thread_rng();
        let keypair = Keypair::new();
        let ping = Ping::<32>::new(rng.gen(), &keypair);
        assert!(ping.verify());
        assert!(ping.sanitize().is_ok());

        let pong = Pong::new(&ping, &keypair);
        assert!(pong.verify());
        assert!(pong.sanitize().is_ok());
        assert_eq!(
            solana_sdk::hash::hashv(&[PING_PONG_HASH_PREFIX, &ping.token]),
            pong.hash
        );
    }

    #[test]
    fn test_ping_cache() {
        let now = Instant::now();
        let mut rng = rand::thread_rng();
        let ttl = Duration::from_millis(256);
        let delay = ttl / 64;
        let mut cache = PingCache::new(&mut rng, Instant::now(), ttl, delay, /*cap=*/ 1000);
        let this_node = Keypair::new();
        let keypairs: Vec<_> = repeat_with(Keypair::new).take(8).collect();
        let sockets: Vec<_> = repeat_with(|| {
            SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(rng.gen(), rng.gen(), rng.gen(), rng.gen()),
                rng.gen(),
            ))
        })
        .take(8)
        .collect();
        let remote_nodes: Vec<(&Keypair, SocketAddr)> = repeat_with(|| {
            let keypair = &keypairs[rng.gen_range(0..keypairs.len())];
            let socket = sockets[rng.gen_range(0..sockets.len())];
            (keypair, socket)
        })
        .take(128)
        .collect();

        // Initially all checks should fail. The first observation of each node
        // should create a ping packet.
        let mut seen_nodes = HashSet::<(Pubkey, SocketAddr)>::new();
        let pings: Vec<Option<Ping<32>>> = remote_nodes
            .iter()
            .map(|(keypair, socket)| {
                let node = (keypair.pubkey(), *socket);
                let (check, ping) = cache.check(&mut rng, &this_node, now, node);
                assert!(!check);
                assert_eq!(seen_nodes.insert(node), ping.is_some());
                ping
            })
            .collect();

        let now = now + Duration::from_millis(1);
        for ((keypair, socket), ping) in remote_nodes.iter().zip(&pings) {
            match ping {
                None => {
                    // Already have a recent ping packets for nodes, so no new
                    // ping packet will be generated.
                    let node = (keypair.pubkey(), *socket);
                    let (check, ping) = cache.check(&mut rng, &this_node, now, node);
                    assert!(check);
                    assert!(ping.is_none());
                }
                Some(ping) => {
                    let pong = Pong::new(ping, keypair);
                    assert!(cache.add(&pong, *socket, now));
                }
            }
        }

        let now = now + Duration::from_millis(1);
        // All nodes now have a recent pong packet.
        for (keypair, socket) in &remote_nodes {
            let node = (keypair.pubkey(), *socket);
            let (check, ping) = cache.check(&mut rng, &this_node, now, node);
            assert!(check);
            assert!(ping.is_none());
        }

        let now = now + ttl / 8;
        // All nodes still have a valid pong packet, but the cache will create
        // a new ping packet to extend verification.
        seen_nodes.clear();
        for (keypair, socket) in &remote_nodes {
            let node = (keypair.pubkey(), *socket);
            let (check, ping) = cache.check(&mut rng, &this_node, now, node);
            assert!(check);
            assert_eq!(seen_nodes.insert(node), ping.is_some());
        }

        let now = now + Duration::from_millis(1);
        // All nodes still have a valid pong packet, and a very recent ping
        // packet pending response. So no new ping packet will be created.
        for (keypair, socket) in &remote_nodes {
            let node = (keypair.pubkey(), *socket);
            let (check, ping) = cache.check(&mut rng, &this_node, now, node);
            assert!(check);
            assert!(ping.is_none());
        }

        let now = now + ttl;
        // Pong packets are still valid but expired. The first observation of
        // each node will remove the pong packet from cache and create a new
        // ping packet.
        seen_nodes.clear();
        for (keypair, socket) in &remote_nodes {
            let node = (keypair.pubkey(), *socket);
            let (check, ping) = cache.check(&mut rng, &this_node, now, node);
            if seen_nodes.insert(node) {
                assert!(check);
                assert!(ping.is_some());
            } else {
                assert!(!check);
                assert!(ping.is_none());
            }
        }

        let now = now + Duration::from_millis(1);
        // No valid pong packet in the cache. A recent ping packet already
        // created, so no new one will be created.
        for (keypair, socket) in &remote_nodes {
            let node = (keypair.pubkey(), *socket);
            let (check, ping) = cache.check(&mut rng, &this_node, now, node);
            assert!(!check);
            assert!(ping.is_none());
        }

        let now = now + ttl / 64;
        // No valid pong packet in the cache. Another ping packet will be
        // created for the first observation of each node.
        seen_nodes.clear();
        for (keypair, socket) in &remote_nodes {
            let node = (keypair.pubkey(), *socket);
            let (check, ping) = cache.check(&mut rng, &this_node, now, node);
            assert!(!check);
            assert_eq!(seen_nodes.insert(node), ping.is_some());
        }
    }
}
