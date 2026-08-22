//! Key routing across servers.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

/// Picks the server for a key. `servers` lists one identity per configured
/// server, in configuration order; the result is an index into it.
///
/// The identity is the address as configured (see [`ServerAddress`]),
/// not what it resolved to, so routing is the same in every process and
/// over time: a hostname that resolves differently on two machines, or
/// to a different family first, still routes every key the same way.
///
/// The default is [`Rendezvous`]. Implement this to plug in weights,
/// health awareness or a different distribution.
pub trait Router: Send + Sync + 'static {
    fn route(&self, key: &[u8], servers: &[String]) -> usize;
}

/// A server address as configured: anything the clients accept to
/// connect, with a stable textual identity for the [`Router`]. Socket
/// addresses and `host:port` strings render the same way, so a server
/// given as `"127.0.0.1:11211"` or as the equivalent [`SocketAddr`] is
/// routed identically.
pub trait ServerAddress {
    /// The identity handed to the [`Router`].
    fn identity(&self) -> String;
}

impl ServerAddress for str {
    fn identity(&self) -> String {
        self.to_owned()
    }
}

impl ServerAddress for String {
    fn identity(&self) -> String {
        self.clone()
    }
}

impl<T: ServerAddress + ?Sized> ServerAddress for &T {
    fn identity(&self) -> String {
        (**self).identity()
    }
}

macro_rules! impl_server_address_display {
    ($($ty:ty),*) => {$(
        impl ServerAddress for $ty {
            fn identity(&self) -> String {
                self.to_string()
            }
        }
    )*};
}

impl_server_address_display!(SocketAddr, SocketAddrV4, SocketAddrV6);

macro_rules! impl_server_address_pair {
    ($($host:ty),*) => {$(
        impl ServerAddress for ($host, u16) {
            fn identity(&self) -> String {
                SocketAddr::from((self.0, self.1)).to_string()
            }
        }
    )*};
}

impl_server_address_pair!(IpAddr, Ipv4Addr, Ipv6Addr);

impl ServerAddress for (&str, u16) {
    fn identity(&self) -> String {
        format!("{}:{}", self.0, self.1)
    }
}

impl ServerAddress for (String, u16) {
    fn identity(&self) -> String {
        format!("{}:{}", self.0, self.1)
    }
}

/// The default key hash: [`DefaultHasher`] over the key bytes.
pub fn default_hash_function(key: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// Rendezvous (highest random weight) hashing: every key scores each
/// server by mixing the key hash with the server identity and picks the
/// highest. Adding or removing a server, anywhere in the list, only moves
/// the keys that belonged to it; the list order does not matter.
#[derive(Clone, Copy)]
pub struct Rendezvous {
    hash_function: fn(&[u8]) -> u64,
}

impl Rendezvous {
    /// Rendezvous over [`default_hash_function`].
    pub fn new() -> Rendezvous {
        Rendezvous::with_hash_function(default_hash_function)
    }

    /// Rendezvous over a custom key hash (to match another client's
    /// distribution, for example).
    pub fn with_hash_function(hash_function: fn(&[u8]) -> u64) -> Rendezvous {
        Rendezvous { hash_function }
    }
}

impl Default for Rendezvous {
    fn default() -> Rendezvous {
        Rendezvous::new()
    }
}

impl std::fmt::Debug for Rendezvous {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rendezvous")
    }
}

/// A 64-bit finalizer (from SplitMix64) to spread the combined hash.
fn mix(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

impl Router for Rendezvous {
    fn route(&self, key: &[u8], servers: &[String]) -> usize {
        let key_hash = (self.hash_function)(key);
        let mut best = (0, 0u64);
        for (index, server) in servers.iter().enumerate() {
            let mut hasher = DefaultHasher::new();
            server.as_bytes().hash(&mut hasher);
            let score = mix(key_hash ^ hasher.finish());
            if index == 0 || score > best.1 {
                best = (index, score);
            }
        }
        best.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn servers(n: u16) -> Vec<String> {
        (0..n).map(|i| format!("10.0.0.1:{}", 11211 + i)).collect()
    }

    #[test]
    fn identities_agree_across_address_forms() {
        let addr: SocketAddr = "127.0.0.1:11211".parse().unwrap();
        assert_eq!("127.0.0.1:11211".identity(), addr.identity());
        assert_eq!((Ipv4Addr::LOCALHOST, 11211).identity(), addr.identity());
        assert_eq!(("localhost", 11211).identity(), "localhost:11211");
        assert_eq!(String::from("[::1]:11211").identity(), "[::1]:11211");
    }

    #[test]
    fn rendezvous_balances_and_moves_minimally() {
        let router = Rendezvous::new();
        let four = servers(4);
        let mut counts = [0usize; 4];
        let mut moved = 0;
        for key in 0..4000u64 {
            let key = key.to_le_bytes();
            let before = router.route(&key, &four);
            counts[before] += 1;
            // Removing the server in the middle only moves its own keys.
            let mut three = four.clone();
            three.remove(1);
            let after = router.route(&key, &three);
            if before == 1 {
                moved += 1;
            } else {
                let expected = if before > 1 { before - 1 } else { before };
                assert_eq!(after, expected);
            }
        }
        for &count in &counts {
            assert!(count > 700, "unbalanced: {counts:?}");
        }
        assert_eq!(moved, counts[1]);
        assert_eq!(router.route(b"k", &servers(1)), 0);
    }
}
