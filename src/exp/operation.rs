//! Operation descriptions for the semantic layer.
//!
//! An operation captures *what* the caller wants (key, value, conditions,
//! requested metadata) independently of the wire encoding. The core layer
//! turns operations into [`MetaCommand`](super::MetaCommand)s and pairs them
//! with their responses. Construct with `new` and adjust fields with struct
//! update syntax: `Get { touch: Some(60), ..Get::new("foo") }`.

use super::meta_api::{ArithmeticMode, SetMode};

/// Which item metadata a [`Get`] should fetch into
/// [`ItemMeta`](super::ItemMeta).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
}

/// A read operation (`mg`).
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub fn new(key: impl Into<Vec<u8>>) -> Get {
        Get {
            key: key.into(),
            meta: Meta::NONE,
            touch: None,
            no_lru_bump: false,
            unless_cas: None,
            value: true,
            lease_ttl: None,
            refresh_before: None,
        }
    }
}

/// A store operation (`ms`). The value is raw bytes; serialization belongs
/// to a higher layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Set {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub ttl: Option<u32>,
    pub mode: SetMode,
    /// Store only when the item CAS matches.
    pub compare_cas: Option<u64>,
    /// Replace the item CAS with this value instead of a server-chosen one.
    pub version: Option<u64>,
    /// Return the new item CAS in the result.
    pub return_cas: bool,
    /// For append/prepend, vivify a missing item with this TTL. Must be >= 1.
    pub vivify_ttl: Option<u32>,
}

impl Set {
    pub fn new(key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Set {
        Set {
            key: key.into(),
            value: value.into(),
            ttl: None,
            mode: SetMode::Set,
            compare_cas: None,
            version: None,
            return_cas: false,
            vivify_ttl: None,
        }
    }
}

/// A delete operation (`md`).
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub fn new(key: impl Into<Vec<u8>>) -> Delete {
        Delete {
            key: key.into(),
            compare_cas: None,
            invalidate: false,
            stale_for: None,
        }
    }
}

/// A counter operation (`ma`).
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Replace the item CAS with this value instead of a server-chosen one.
    pub version: Option<u64>,
    /// Return the new item CAS in the result.
    pub return_cas: bool,
    /// Return the remaining TTL in the result.
    pub return_ttl: bool,
}

impl Arithmetic {
    pub fn new(key: impl Into<Vec<u8>>) -> Arithmetic {
        Arithmetic {
            key: key.into(),
            delta: 1,
            mode: ArithmeticMode::Increment,
            initial: None,
            initial_ttl: None,
            ttl: None,
            compare_cas: None,
            version: None,
            return_cas: false,
            return_ttl: false,
        }
    }
}
