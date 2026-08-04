//! Typed results for the semantic layer.
//!
//! Results describe protocol-level outcomes (miss, CAS mismatch, lease
//! state); transport and parse failures surface as
//! [`MemcacheError`](crate::MemcacheError) instead. Batch-only outcomes
//! (failed/ambiguous operations inside a pipeline) will be added together
//! with the pipeline executor.

use crate::error::MemcacheError;

use super::value::FromValue;

/// Outcome of a [`Get`](super::Get) operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GetStatus {
    /// The item was found (the value is present when it was requested).
    Hit,
    /// The item was not found.
    Miss,
    /// The item is a placeholder another client is currently filling.
    Pending,
    /// `unless_cas` matched, so the server suppressed the value.
    Unchanged,
}

/// Outcome of a mutation ([`Set`](super::Set) / [`Delete`](super::Delete) /
/// [`Arithmetic`](super::Arithmetic)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MutationStatus {
    /// The mutation took effect: the value was stored, the item deleted,
    /// or the counter adjusted.
    Applied,
    /// The item does not exist.
    NotFound,
    /// Add mode: the item already exists.
    AlreadyExists,
    /// `compare_cas` did not match the item.
    CasMismatch,
}

/// Freshness of a returned value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValueState {
    Fresh,
    /// The item was invalidated and awaits a refresh.
    Stale,
    Missing,
}

/// Lease outcome of a [`Get`](super::Get) with `lease_ttl`/`refresh_before`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LeaseState {
    /// No lease was requested or granted.
    None,
    /// This client won the lease and should recompute the value.
    Granted,
    /// Another client holds the lease.
    Busy,
}

/// Item metadata requested via [`Meta`](super::Meta); a field is `None` when
/// it was not requested or the server did not send it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ItemMeta {
    pub cas: Option<u64>,
    /// Remaining TTL in seconds; `-1` means unlimited.
    pub ttl: Option<i64>,
    pub size: Option<u64>,
    pub last_access: Option<u64>,
    pub hit_before: Option<bool>,
}

/// Result of a [`Get`](super::Get) operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GetResult {
    pub key: Vec<u8>,
    pub status: GetStatus,
    /// The raw value; present only on a [`Hit`](GetStatus::Hit) that
    /// requested the value.
    pub value: Option<Vec<u8>>,
    /// The client flags stored with the item, used by
    /// [`decode`](Self::decode).
    pub client_flags: Option<u32>,
    pub item: ItemMeta,
    pub value_state: ValueState,
    pub lease_state: LeaseState,
}

impl GetResult {
    pub fn hit(&self) -> bool {
        self.status == GetStatus::Hit
    }

    /// Decode the value into the requested type via
    /// [`FromValue`](super::FromValue); `Ok(None)` when there is no value.
    ///
    /// ```no_run
    /// # use memcache::exp::MetaClient;
    /// # let mut client = MetaClient::connect("127.0.0.1:11211").unwrap();
    /// let count: Option<u64> = client.get("hits").send().unwrap().decode().unwrap();
    /// ```
    pub fn decode<V: FromValue>(self) -> Result<Option<V>, MemcacheError> {
        let flags = self.client_flags.unwrap_or(0);
        self.value.map(|value| V::from_value(value, flags)).transpose()
    }

    pub fn is_stale(&self) -> bool {
        self.value_state == ValueState::Stale
    }

    /// This client won the lease and should recompute the value.
    pub fn won_lease(&self) -> bool {
        self.lease_state == LeaseState::Granted
    }

    /// Another client already holds the lease.
    pub fn lease_busy(&self) -> bool {
        self.lease_state == LeaseState::Busy
    }
}

/// Result of a [`Set`](super::Set) or [`Delete`](super::Delete) operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MutationResult {
    pub key: Vec<u8>,
    pub status: MutationStatus,
    /// The new item CAS, when requested with `return_cas`.
    pub cas: Option<u64>,
}

impl MutationResult {
    /// Whether the mutation took effect.
    pub fn applied(&self) -> bool {
        self.status == MutationStatus::Applied
    }
}

/// Result of an [`Arithmetic`](super::Arithmetic) operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ArithmeticResult {
    pub key: Vec<u8>,
    pub status: MutationStatus,
    /// The counter value after the operation, unless suppressed.
    pub value: Option<u64>,
    pub item: ItemMeta,
}

impl ArithmeticResult {
    /// Whether the delta was applied (or the counter vivified).
    pub fn applied(&self) -> bool {
        self.status == MutationStatus::Applied
    }
}

/// Result of an [`Op`](super::Op) in a batch, one variant per operation
/// kind ([`Set`](super::Set) and [`Delete`](super::Delete) both yield
/// `Mutation`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpResult {
    Get(GetResult),
    Mutation(MutationResult),
    Arithmetic(ArithmeticResult),
}

impl OpResult {
    pub fn as_get(&self) -> Option<&GetResult> {
        match self {
            OpResult::Get(result) => Some(result),
            _ => None,
        }
    }

    pub fn as_mutation(&self) -> Option<&MutationResult> {
        match self {
            OpResult::Mutation(result) => Some(result),
            _ => None,
        }
    }

    pub fn as_arithmetic(&self) -> Option<&ArithmeticResult> {
        match self {
            OpResult::Arithmetic(result) => Some(result),
            _ => None,
        }
    }
}
