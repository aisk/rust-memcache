//! Operation descriptions for the semantic layer.
//!
//! An operation captures *what* the caller wants (key, value, conditions,
//! requested metadata) independently of the wire encoding. The core layer
//! turns operations into [`MetaCommand`](super::MetaCommand)s and pairs them
//! with their responses. Construct with `new` and chain builder methods:
//! `Get::new("foo").touch(60).lease_ttl(30)`. The structs are
//! `#[non_exhaustive]` so new options can be added without breaking callers;
//! the fields stay public for reading and mutation.

use super::meta_api::{ArithmeticMode, SetMode};
use super::value::ToValue;

/// Which item metadata a [`Get`] should fetch into
/// [`ItemMeta`](super::ItemMeta).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Meta {
    pub cas: bool,
    pub ttl: bool,
    pub size: bool,
    pub last_access: bool,
    pub hit_before: bool,
}

impl Meta {
    pub const NONE: Meta = Meta {
        cas: false,
        ttl: false,
        size: false,
        last_access: false,
        hit_before: false,
    };
    pub const ALL: Meta = Meta {
        cas: true,
        ttl: true,
        size: true,
        last_access: true,
        hit_before: true,
    };

    #[must_use]
    pub fn cas(mut self) -> Meta {
        self.cas = true;
        self
    }

    #[must_use]
    pub fn ttl(mut self) -> Meta {
        self.ttl = true;
        self
    }

    #[must_use]
    pub fn size(mut self) -> Meta {
        self.size = true;
        self
    }

    #[must_use]
    pub fn last_access(mut self) -> Meta {
        self.last_access = true;
        self
    }

    #[must_use]
    pub fn hit_before(mut self) -> Meta {
        self.hit_before = true;
        self
    }
}

/// A read operation (`mg`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Get {
    pub key: Vec<u8>,
    /// Metadata to fetch alongside the value.
    pub meta: Meta,
    /// Update the item TTL while reading.
    pub touch: Option<u32>,
    /// Don't bump the item in the LRU.
    pub no_lru_bump: bool,
    /// Suppress the value when the item CAS still matches; the result status
    /// becomes [`Unchanged`](super::GetStatus::Unchanged). Requires `value`.
    pub unless_cas: Option<u64>,
    /// Whether to read the value at all; `false` fetches metadata only.
    pub value: bool,
    /// Vivify a missing key with this TTL and request a lease; a miss then
    /// reports [`LeaseState::Granted`](super::LeaseState::Granted) to exactly
    /// one client. Must be >= 1.
    pub lease_ttl: Option<u32>,
    /// Also win the lease when the remaining TTL drops below this, to
    /// refresh the value before it expires. Requires `lease_ttl`; must
    /// be >= 1.
    pub refresh_before: Option<u32>,
}

impl Get {
    pub fn new(key: impl AsRef<[u8]>) -> Get {
        Get {
            key: key.as_ref().to_vec(),
            meta: Meta::NONE,
            touch: None,
            no_lru_bump: false,
            unless_cas: None,
            value: true,
            lease_ttl: None,
            refresh_before: None,
        }
    }

    /// Fetch this metadata alongside the value.
    #[must_use]
    pub fn meta(mut self, meta: Meta) -> Get {
        self.meta = meta;
        self
    }

    /// Update the item TTL while reading.
    #[must_use]
    pub fn touch(mut self, ttl: u32) -> Get {
        self.touch = Some(ttl);
        self
    }

    /// Don't bump the item in the LRU.
    #[must_use]
    pub fn no_lru_bump(mut self) -> Get {
        self.no_lru_bump = true;
        self
    }

    /// Suppress the value when the item CAS still matches.
    #[must_use]
    pub fn unless_cas(mut self, cas: u64) -> Get {
        self.unless_cas = Some(cas);
        self
    }

    /// Fetch metadata only, without the value.
    #[must_use]
    pub fn without_value(mut self) -> Get {
        self.value = false;
        self
    }

    /// Vivify a missing key with this TTL and request a lease.
    #[must_use]
    pub fn lease_ttl(mut self, ttl: u32) -> Get {
        self.lease_ttl = Some(ttl);
        self
    }

    /// Also win the lease when the remaining TTL drops below this.
    #[must_use]
    pub fn refresh_before(mut self, ttl: u32) -> Get {
        self.refresh_before = Some(ttl);
        self
    }
}

/// A store operation (`ms`). The value is encoded up front via
/// [`ToValue`]; `value` holds the raw bytes and `client_flags` the flags
/// stored with the item.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Set {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    /// `F<flags>` - client flags stored with the item, from [`ToValue`].
    pub client_flags: u32,
    pub ttl: Option<u32>,
    pub mode: SetMode,
    /// Store only when the item CAS matches.
    pub compare_cas: Option<u64>,
    /// Overwrite the item CAS with this value instead of a server-chosen one
    /// (protocol `E` flag) - for replicating or restoring items with a known
    /// CAS, not for normal CAS loops (use `compare_cas`).
    pub force_cas: Option<u64>,
    /// Return the new item CAS in the result.
    pub return_cas: bool,
    /// For append/prepend, vivify a missing item with this TTL. Must be >= 1.
    pub vivify_ttl: Option<u32>,
}

impl Set {
    pub fn new(key: impl AsRef<[u8]>, value: impl ToValue) -> Set {
        let (value, client_flags) = value.to_value();
        Set {
            key: key.as_ref().to_vec(),
            value,
            client_flags,
            ttl: None,
            mode: SetMode::Set,
            compare_cas: None,
            force_cas: None,
            return_cas: false,
            vivify_ttl: None,
        }
    }

    #[must_use]
    pub fn ttl(mut self, ttl: u32) -> Set {
        self.ttl = Some(ttl);
        self
    }

    /// Override the client flags stored with the item (normally chosen by
    /// [`ToValue`]) - for interop with other clients' flag conventions.
    #[must_use]
    pub fn client_flags(mut self, flags: u32) -> Set {
        self.client_flags = flags;
        self
    }

    #[must_use]
    pub fn mode(mut self, mode: SetMode) -> Set {
        self.mode = mode;
        self
    }

    /// Store only when the item does not exist.
    #[must_use]
    pub fn add(self) -> Set {
        self.mode(SetMode::Add)
    }

    /// Store only when the item exists.
    #[must_use]
    pub fn replace(self) -> Set {
        self.mode(SetMode::Replace)
    }

    /// Append raw bytes to the stored value.
    #[must_use]
    pub fn append(self) -> Set {
        self.mode(SetMode::Append)
    }

    /// Prepend raw bytes to the stored value.
    #[must_use]
    pub fn prepend(self) -> Set {
        self.mode(SetMode::Prepend)
    }

    /// Store only when the item CAS matches.
    #[must_use]
    pub fn compare_cas(mut self, cas: u64) -> Set {
        self.compare_cas = Some(cas);
        self
    }

    /// Overwrite the item CAS with this value (protocol `E` flag) - for
    /// replicating or restoring items, not for normal CAS loops.
    #[must_use]
    pub fn force_cas(mut self, cas: u64) -> Set {
        self.force_cas = Some(cas);
        self
    }

    /// Return the new item CAS in the result.
    #[must_use]
    pub fn return_cas(mut self) -> Set {
        self.return_cas = true;
        self
    }

    /// For append/prepend, vivify a missing item with this TTL.
    #[must_use]
    pub fn vivify_ttl(mut self, ttl: u32) -> Set {
        self.vivify_ttl = Some(ttl);
        self
    }
}

/// A delete operation (`md`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Delete {
    pub key: Vec<u8>,
    /// Delete only when the item CAS matches.
    pub compare_cas: Option<u64>,
    /// Mark the item stale instead of removing it; readers then see the old
    /// value flagged stale until someone refreshes it.
    pub invalidate: bool,
    /// For invalidate, how long the stale item stays readable.
    pub stale_for: Option<u32>,
}

impl Delete {
    pub fn new(key: impl AsRef<[u8]>) -> Delete {
        Delete {
            key: key.as_ref().to_vec(),
            compare_cas: None,
            invalidate: false,
            stale_for: None,
        }
    }

    /// Delete only when the item CAS matches.
    #[must_use]
    pub fn compare_cas(mut self, cas: u64) -> Delete {
        self.compare_cas = Some(cas);
        self
    }

    /// Mark the item stale instead of removing it.
    #[must_use]
    pub fn invalidate(mut self) -> Delete {
        self.invalidate = true;
        self
    }

    /// For invalidate, how long the stale item stays readable.
    #[must_use]
    pub fn stale_for(mut self, ttl: u32) -> Delete {
        self.stale_for = Some(ttl);
        self
    }
}

/// A counter operation (`ma`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Arithmetic {
    pub key: Vec<u8>,
    pub delta: u64,
    pub mode: ArithmeticMode,
    /// Initial value when vivifying a missing item; requires `initial_ttl`.
    pub initial: Option<u64>,
    /// TTL for the vivified item; requires `initial`. Must be >= 1.
    pub initial_ttl: Option<u32>,
    /// Update the item TTL while applying the delta.
    pub ttl: Option<u32>,
    /// Apply only when the item CAS matches.
    pub compare_cas: Option<u64>,
    /// Overwrite the item CAS with this value instead of a server-chosen one
    /// (protocol `E` flag) - for replicating or restoring items with a known
    /// CAS, not for normal CAS loops (use `compare_cas`).
    pub force_cas: Option<u64>,
    /// Return the new item CAS in the result.
    pub return_cas: bool,
    /// Return the remaining TTL in the result.
    pub return_ttl: bool,
}

impl Arithmetic {
    pub fn new(key: impl AsRef<[u8]>) -> Arithmetic {
        Arithmetic {
            key: key.as_ref().to_vec(),
            delta: 1,
            mode: ArithmeticMode::Increment,
            initial: None,
            initial_ttl: None,
            ttl: None,
            compare_cas: None,
            force_cas: None,
            return_cas: false,
            return_ttl: false,
        }
    }

    #[must_use]
    pub fn delta(mut self, delta: u64) -> Arithmetic {
        self.delta = delta;
        self
    }

    /// Decrement instead of increment.
    #[must_use]
    pub fn decrement(mut self) -> Arithmetic {
        self.mode = ArithmeticMode::Decrement;
        self
    }

    /// Vivify a missing item with this value and TTL. The protocol requires
    /// the pair, so they are set together.
    #[must_use]
    pub fn initial(mut self, value: u64, ttl: u32) -> Arithmetic {
        self.initial = Some(value);
        self.initial_ttl = Some(ttl);
        self
    }

    /// Update the item TTL while applying the delta.
    #[must_use]
    pub fn ttl(mut self, ttl: u32) -> Arithmetic {
        self.ttl = Some(ttl);
        self
    }

    /// Apply only when the item CAS matches.
    #[must_use]
    pub fn compare_cas(mut self, cas: u64) -> Arithmetic {
        self.compare_cas = Some(cas);
        self
    }

    /// Overwrite the item CAS with this value (protocol `E` flag) - for
    /// replicating or restoring items, not for normal CAS loops.
    #[must_use]
    pub fn force_cas(mut self, cas: u64) -> Arithmetic {
        self.force_cas = Some(cas);
        self
    }

    /// Return the new item CAS in the result.
    #[must_use]
    pub fn return_cas(mut self) -> Arithmetic {
        self.return_cas = true;
        self
    }

    /// Return the remaining TTL in the result.
    #[must_use]
    pub fn return_ttl(mut self) -> Arithmetic {
        self.return_ttl = true;
        self
    }
}

/// Any operation, for heterogeneous batches: `client.run_batch([op.into(),
/// ...])`. Running an `Op` yields an [`OpResult`](super::OpResult).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Op {
    Get(Get),
    Set(Set),
    Delete(Delete),
    Arithmetic(Arithmetic),
}

impl From<Get> for Op {
    fn from(operation: Get) -> Op {
        Op::Get(operation)
    }
}

impl From<Set> for Op {
    fn from(operation: Set) -> Op {
        Op::Set(operation)
    }
}

impl From<Delete> for Op {
    fn from(operation: Delete) -> Op {
        Op::Delete(operation)
    }
}

impl From<Arithmetic> for Op {
    fn from(operation: Arithmetic) -> Op {
        Op::Arithmetic(operation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_set_fields() {
        let get = Get::new("foo")
            .meta(Meta::NONE.cas().ttl())
            .touch(60)
            .no_lru_bump()
            .lease_ttl(30)
            .refresh_before(10);
        assert!(get.meta.cas && get.meta.ttl && !get.meta.size);
        assert_eq!(get.touch, Some(60));
        assert!(get.no_lru_bump);
        assert_eq!(get.lease_ttl, Some(30));
        assert_eq!(get.refresh_before, Some(10));
        assert!(!Get::new("foo").without_value().value);

        let set = Set::new("foo", "bar")
            .ttl(60)
            .add()
            .compare_cas(7)
            .return_cas()
            .client_flags(9);
        assert_eq!(set.ttl, Some(60));
        assert_eq!(set.mode, SetMode::Add);
        assert_eq!(set.compare_cas, Some(7));
        assert!(set.return_cas);
        assert_eq!(set.client_flags, 9);

        let delete = Delete::new("foo").invalidate().stale_for(30);
        assert!(delete.invalidate);
        assert_eq!(delete.stale_for, Some(30));

        let arithmetic = Arithmetic::new("counter").delta(2).decrement().initial(0, 60);
        assert_eq!(arithmetic.delta, 2);
        assert_eq!(arithmetic.mode, ArithmeticMode::Decrement);
        assert_eq!(arithmetic.initial, Some(0));
        assert_eq!(arithmetic.initial_ttl, Some(60));
    }
}
