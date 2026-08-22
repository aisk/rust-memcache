//! Key routing across servers.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;

/// Picks the server for a key. `servers` lists one identifying address
/// per configured server (its first resolved address), in configuration
/// order; the result is an index into it.
///
/// The default is [`Rendezvous`]. Implement this to plug in weights,
/// health awareness or a different distribution.
pub trait Router: Send + Sync + 'static {
    fn route(&self, key: &[u8], servers: &[SocketAddr]) -> usize;
}

/// The default key hash: [`DefaultHasher`] over the key bytes.
pub fn default_hash_function(key: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// Rendezvous (highest random weight) hashing: every key scores each
/// server by mixing the key hash with the server address and picks the
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
    fn route(&self, key: &[u8], servers: &[SocketAddr]) -> usize {
        let key_hash = (self.hash_function)(key);
        let mut best = (0, 0u64);
        for (index, server) in servers.iter().enumerate() {
            let mut hasher = DefaultHasher::new();
            server.hash(&mut hasher);
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

    fn servers(n: u16) -> Vec<SocketAddr> {
        (0..n).map(|i| SocketAddr::from(([10, 0, 0, 1], 11211 + i))).collect()
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
