//! Transport-independent semantic core: turns operations into wire commands
//! and pairs wire results back with their operations to produce typed
//! results. Both clients (blocking and tokio) are thin I/O loops around
//! these functions, and the future pipeline executor will reuse them.

use super::error::{Error, Result};

use super::meta_api::{
    ArithmeticOptions, DeleteOptions, GetOptions, MetaCommandResult, SetMode, SetOptions, build_arithmetic,
    build_delete, build_get, build_set,
};
use super::meta_command::{MetaCommand, ReturnCode};
use super::operation::{Arithmetic, Delete, Get, Op, Set};
use super::result::{
    ArithmeticResult, GetResult, GetStatus, ItemMeta, LeaseState, MutationResult, MutationStatus, OpResult, ValueState,
};
use super::ttl::Ttl;

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Get {}
    impl Sealed for super::Set {}
    impl Sealed for super::Delete {}
    impl Sealed for super::Arithmetic {}
    impl Sealed for super::Op {}
}

/// An executable semantic-layer operation, implemented by [`Get`], [`Set`],
/// [`Delete`] and [`Arithmetic`]. Sealed: not implementable outside this
/// crate.
pub trait Operation: sealed::Sealed {
    /// The typed result this operation produces.
    type Output;

    #[doc(hidden)]
    fn key(&self) -> &[u8];

    #[doc(hidden)]
    fn prepare(&self) -> Result<MetaCommand>;

    #[doc(hidden)]
    fn parse(&self, wire: MetaCommandResult) -> Result<Self::Output>;
}

impl Operation for Get {
    type Output = GetResult;

    fn key(&self) -> &[u8] {
        &self.key
    }

    fn prepare(&self) -> Result<MetaCommand> {
        prepare_get(self)
    }

    fn parse(&self, wire: MetaCommandResult) -> Result<GetResult> {
        parse_get(self, wire)
    }
}

impl Operation for Set {
    type Output = MutationResult;

    fn key(&self) -> &[u8] {
        &self.key
    }

    fn prepare(&self) -> Result<MetaCommand> {
        prepare_set(self)
    }

    fn parse(&self, wire: MetaCommandResult) -> Result<MutationResult> {
        parse_set(self, wire)
    }
}

impl Operation for Delete {
    type Output = MutationResult;

    fn key(&self) -> &[u8] {
        &self.key
    }

    fn prepare(&self) -> Result<MetaCommand> {
        prepare_delete(self)
    }

    fn parse(&self, wire: MetaCommandResult) -> Result<MutationResult> {
        parse_delete(self, wire)
    }
}

impl Operation for Arithmetic {
    type Output = ArithmeticResult;

    fn key(&self) -> &[u8] {
        &self.key
    }

    fn prepare(&self) -> Result<MetaCommand> {
        prepare_arithmetic(self)
    }

    fn parse(&self, wire: MetaCommandResult) -> Result<ArithmeticResult> {
        parse_arithmetic(self, wire)
    }
}

impl Operation for Op {
    type Output = OpResult;

    fn key(&self) -> &[u8] {
        match self {
            Op::Get(operation) => &operation.key,
            Op::Set(operation) => &operation.key,
            Op::Delete(operation) => &operation.key,
            Op::Arithmetic(operation) => &operation.key,
        }
    }

    fn prepare(&self) -> Result<MetaCommand> {
        match self {
            Op::Get(operation) => operation.prepare(),
            Op::Set(operation) => operation.prepare(),
            Op::Delete(operation) => operation.prepare(),
            Op::Arithmetic(operation) => operation.prepare(),
        }
    }

    fn parse(&self, wire: MetaCommandResult) -> Result<OpResult> {
        match self {
            Op::Get(operation) => operation.parse(wire).map(OpResult::Get),
            Op::Set(operation) => operation.parse(wire).map(OpResult::Mutation),
            Op::Delete(operation) => operation.parse(wire).map(OpResult::Mutation),
            Op::Arithmetic(operation) => operation.parse(wire).map(OpResult::Arithmetic),
        }
    }
}

fn invalid<T>(message: &'static str) -> Result<T> {
    Err(Error::Usage(message))
}

/// A validated batch: every operation prepared into a wire command, and the
/// operation indexes grouped per server. Shared by both clients.
pub(crate) struct BatchPlan {
    /// One prepared command per operation; entries are taken as groups
    /// execute.
    pub(crate) commands: Vec<Option<MetaCommand>>,
    /// Operation indexes per server, in input order.
    pub(crate) groups: Vec<Vec<usize>>,
}

/// Prepare and group a batch. Validates every operation before anything is
/// written, so a bad option never leaves half a batch on the wire.
pub(crate) fn plan<O: Operation>(
    operations: &[O],
    servers: usize,
    index_for: impl Fn(&[u8]) -> usize,
) -> Result<BatchPlan> {
    let mut commands = Vec::with_capacity(operations.len());
    for operation in operations {
        commands.push(Some(operation.prepare()?));
    }
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); servers];
    for (index, operation) in operations.iter().enumerate() {
        groups[index_for(operation.key())].push(index);
    }
    Ok(BatchPlan { commands, groups })
}

fn unexpected<T>(message: &'static str) -> Result<T> {
    Err(Error::protocol(message))
}

pub(crate) fn prepare_get(operation: &Get) -> Result<MetaCommand> {
    if operation.lease_ttl == Some(0) {
        return invalid("lease_ttl must be >= 1");
    }
    if operation.refresh_before == Some(0) {
        return invalid("refresh_before must be >= 1");
    }
    if operation.refresh_before.is_some() && operation.lease_ttl.is_none() {
        return invalid("refresh_before requires lease_ttl");
    }
    if operation.unless_cas.is_some() && !operation.value {
        return invalid("unless_cas requires a value read");
    }
    // Lease and conditional reads need the CAS to act on the result.
    let return_cas = operation.meta.cas || operation.lease_ttl.is_some() || operation.unless_cas.is_some();
    let options = GetOptions {
        value: operation.value,
        return_client_flags: true,
        return_cas,
        return_ttl: operation.meta.ttl,
        return_size: operation.meta.size,
        return_last_access: operation.meta.last_access,
        return_hit_before: operation.meta.hit_before,
        no_lru_bump: operation.no_lru_bump,
        touch: operation.touch.map(Ttl::wire).transpose()?,
        vivify_ttl: operation.lease_ttl,
        recache_ttl: operation.refresh_before,
        unless_cas: operation.unless_cas,
        ..GetOptions::default()
    };
    build_get(operation.key.clone(), &options)
}

pub(crate) fn prepare_set(operation: &Set) -> Result<MetaCommand> {
    if operation.vivify_ttl == Some(0) {
        return invalid("vivify_ttl must be >= 1");
    }
    let options = SetOptions {
        // An omitted F stores flags 0, so only non-zero flags go on the wire.
        client_flags: (operation.client_flags != 0).then_some(operation.client_flags),
        ttl: operation.ttl.map(Ttl::wire).transpose()?,
        mode: operation.mode,
        compare_cas: operation.compare_cas,
        new_cas: operation.force_cas,
        vivify_ttl: operation.vivify_ttl,
        return_cas: operation.return_cas,
        ..SetOptions::default()
    };
    build_set(operation.key.clone(), operation.value.clone(), &options)
}

pub(crate) fn prepare_delete(operation: &Delete) -> Result<MetaCommand> {
    if operation.stale_for.is_some() && !operation.invalidate {
        return invalid("stale_for is only valid for invalidate");
    }
    let options = DeleteOptions {
        compare_cas: operation.compare_cas,
        invalidate: operation.invalidate,
        ttl: operation.stale_for.map(Ttl::wire).transpose()?,
        ..DeleteOptions::default()
    };
    build_delete(operation.key.clone(), &options)
}

pub(crate) fn prepare_arithmetic(operation: &Arithmetic) -> Result<MetaCommand> {
    if operation.initial_ttl == Some(0) {
        return invalid("initial_ttl must be >= 1");
    }
    if operation.initial_ttl.is_some() && operation.initial.is_none() {
        return invalid("initial_ttl requires initial");
    }
    let options = ArithmeticOptions {
        delta: Some(operation.delta),
        mode: operation.mode,
        initial: operation.initial,
        initial_ttl: operation.initial_ttl,
        ttl: operation.ttl.map(Ttl::wire).transpose()?,
        compare_cas: operation.compare_cas,
        new_cas: operation.force_cas,
        return_value: true,
        return_ttl: operation.return_ttl,
        return_cas: operation.return_cas,
        ..ArithmeticOptions::default()
    };
    build_arithmetic(operation.key.clone(), &options)
}

fn mutation_status(is_add: bool, rc: ReturnCode) -> Result<MutationStatus> {
    match rc {
        ReturnCode::Hd | ReturnCode::Va => Ok(MutationStatus::Applied),
        ReturnCode::Ex => Ok(MutationStatus::CasMismatch),
        ReturnCode::Nf => Ok(MutationStatus::NotFound),
        ReturnCode::Ns => Ok(if is_add {
            MutationStatus::AlreadyExists
        } else {
            MutationStatus::NotFound
        }),
        _ => unexpected("unexpected mutation response"),
    }
}

pub(crate) fn parse_get(operation: &Get, wire: MetaCommandResult) -> Result<GetResult> {
    if wire.rc == ReturnCode::En {
        return Ok(GetResult {
            key: operation.key.clone(),
            status: GetStatus::Miss,
            value: None,
            client_flags: None,
            item: ItemMeta::default(),
            value_state: ValueState::Missing,
            lease_state: LeaseState::None,
        });
    }
    if wire.rc != ReturnCode::Va && wire.rc != ReturnCode::Hd {
        return unexpected("unexpected get response");
    }
    let item = ItemMeta {
        cas: wire.cas,
        ttl: wire.ttl,
        size: wire.size,
        last_access: wire.last_access,
        hit_before: wire.hit_before,
    };
    let lease_state = if wire.won {
        LeaseState::Granted
    } else if wire.busy {
        LeaseState::Busy
    } else {
        LeaseState::None
    };
    // A vivified item is an empty non-stale value plus a lease flag: nobody
    // has stored real data yet.
    let placeholder = !wire.stale && wire.value.as_deref() == Some(b"") && lease_state != LeaseState::None;
    let value_state = if wire.stale {
        ValueState::Stale
    } else if placeholder {
        ValueState::Missing
    } else {
        ValueState::Fresh
    };
    let (status, value) = if placeholder {
        // With a lease request the placeholder is this client's own vivify:
        // report a miss and let the caller fill it. Without one, another
        // client is filling it.
        if operation.lease_ttl.is_some() {
            (GetStatus::Miss, None)
        } else {
            (GetStatus::Pending, None)
        }
    } else if wire.rc == ReturnCode::Hd && operation.unless_cas.is_some() {
        (GetStatus::Unchanged, None)
    } else {
        let value = if wire.rc == ReturnCode::Va { wire.value } else { None };
        (GetStatus::Hit, value)
    };
    Ok(GetResult {
        key: operation.key.clone(),
        status,
        value,
        client_flags: wire.client_flags,
        item,
        value_state,
        lease_state,
    })
}

pub(crate) fn parse_set(operation: &Set, wire: MetaCommandResult) -> Result<MutationResult> {
    Ok(MutationResult {
        key: operation.key.clone(),
        status: mutation_status(operation.mode == SetMode::Add, wire.rc)?,
        cas: wire.cas,
    })
}

pub(crate) fn parse_delete(operation: &Delete, wire: MetaCommandResult) -> Result<MutationResult> {
    Ok(MutationResult {
        key: operation.key.clone(),
        status: mutation_status(false, wire.rc)?,
        cas: wire.cas,
    })
}

pub(crate) fn parse_arithmetic(operation: &Arithmetic, wire: MetaCommandResult) -> Result<ArithmeticResult> {
    let status = mutation_status(false, wire.rc)?;
    let value = match (&wire.value, wire.rc) {
        (Some(value), ReturnCode::Va) if !value.is_empty() => Some(
            std::str::from_utf8(value)
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| Error::protocol("arithmetic response is not a number"))?,
        ),
        _ => None,
    };
    Ok(ArithmeticResult {
        key: operation.key.clone(),
        status,
        value,
        item: ItemMeta {
            cas: wire.cas,
            ttl: wire.ttl,
            ..ItemMeta::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::super::meta_api::parse_meta_result;
    use super::super::meta_command::MetaResponse;
    use super::super::operation::Meta;
    use super::*;

    fn wire(header: &[u8], value: Option<&[u8]>) -> MetaCommandResult {
        let mut response = MetaResponse::parse_header(header).unwrap();
        response.value = value.map(|value| value.to_vec());
        parse_meta_result(response).unwrap()
    }

    #[test]
    fn prepare_get_wire_format() {
        let command = prepare_get(&Get::new("foo")).unwrap();
        assert_eq!(command.encode().unwrap(), b"mg foo v f\r\n".to_vec());

        let operation = Get {
            meta: Meta {
                cas: true,
                ttl: true,
                ..Meta::NONE
            },
            touch: Some(Ttl::secs(60)),
            ..Get::new("foo")
        };
        let command = prepare_get(&operation).unwrap();
        assert_eq!(command.encode().unwrap(), b"mg foo v f c t T60\r\n".to_vec());

        // A lease read implies the CAS even when not requested.
        let operation = Get {
            lease_ttl: Some(30),
            ..Get::new("foo")
        };
        let command = prepare_get(&operation).unwrap();
        assert_eq!(command.encode().unwrap(), b"mg foo v f c N30\r\n".to_vec());
    }

    #[test]
    fn prepare_get_validation() {
        assert!(
            prepare_get(&Get {
                lease_ttl: Some(0),
                ..Get::new("k")
            })
            .is_err()
        );
        assert!(
            prepare_get(&Get {
                refresh_before: Some(5),
                ..Get::new("k")
            })
            .is_err()
        );
        assert!(
            prepare_get(&Get {
                unless_cas: Some(1),
                value: false,
                ..Get::new("k")
            })
            .is_err()
        );
    }

    #[test]
    fn prepare_set_wire_format() {
        // The protocol layer stores bytes as given, flags default to zero.
        let command = prepare_set(&Set::new("foo", "bar")).unwrap();
        assert_eq!(command.encode().unwrap(), b"ms foo 3\r\nbar\r\n".to_vec());

        let operation = Set {
            mode: SetMode::Add,
            ttl: Some(Ttl::secs(60)),
            return_cas: true,
            ..Set::new("foo", "bar")
        };
        let command = prepare_set(&operation).unwrap();
        assert_eq!(command.encode().unwrap(), b"ms foo 3 ME c T60\r\nbar\r\n".to_vec());

        // A zero ttl is a usage error, never a silent "keep forever".
        let operation = Set {
            ttl: Some(Ttl::secs(0)),
            ..Set::new("foo", "bar")
        };
        assert!(matches!(prepare_set(&operation), Err(Error::Usage(_))));
        let operation = Set {
            ttl: Some(Ttl::NEVER),
            ..Set::new("foo", "bar")
        };
        assert_eq!(
            prepare_set(&operation).unwrap().encode().unwrap(),
            b"ms foo 3 T0\r\nbar\r\n".to_vec()
        );

        // Non-zero client flags go on the wire as F.
        let operation = Set {
            client_flags: 5,
            ..Set::new("foo", "bar")
        };
        let command = prepare_set(&operation).unwrap();
        assert_eq!(command.encode().unwrap(), b"ms foo 3 F5\r\nbar\r\n".to_vec());
    }

    #[test]
    fn prepare_delete_validation() {
        let command = prepare_delete(&Delete::new("foo")).unwrap();
        assert_eq!(command.encode().unwrap(), b"md foo\r\n".to_vec());
        assert!(
            prepare_delete(&Delete {
                stale_for: Some(Ttl::secs(30)),
                ..Delete::new("foo")
            })
            .is_err()
        );
    }

    #[test]
    fn prepare_arithmetic_validation() {
        let command = prepare_arithmetic(&Arithmetic::new("counter")).unwrap();
        assert_eq!(command.encode().unwrap(), b"ma counter v D1\r\n".to_vec());
        assert!(
            prepare_arithmetic(&Arithmetic {
                initial_ttl: Some(60),
                ..Arithmetic::new("counter")
            })
            .is_err()
        );
        assert!(
            prepare_arithmetic(&Arithmetic {
                initial: Some(0),
                ..Arithmetic::new("counter")
            })
            .is_err()
        );
    }

    #[test]
    fn parse_get_hit_and_miss() {
        let operation = Get::new("foo");
        let result = parse_get(&operation, wire(b"VA 3 f0 c42", Some(b"bar"))).unwrap();
        assert_eq!(result.status, GetStatus::Hit);
        assert!(result.hit());
        assert_eq!(result.value.as_deref(), Some(&b"bar"[..]));
        assert_eq!(result.item.cas, Some(42));
        assert_eq!(result.value_state, ValueState::Fresh);

        let result = parse_get(&operation, wire(b"EN", None)).unwrap();
        assert_eq!(result.status, GetStatus::Miss);
        assert_eq!(result.value, None);

        assert!(parse_get(&operation, wire(b"NS", None)).is_err());
    }

    #[test]
    fn parse_get_lease_placeholder() {
        // Our own vivify: empty placeholder with the won flag is a miss with
        // a granted lease.
        let operation = Get {
            lease_ttl: Some(30),
            ..Get::new("foo")
        };
        let result = parse_get(&operation, wire(b"VA 0 c7 W", Some(b""))).unwrap();
        assert_eq!(result.status, GetStatus::Miss);
        assert!(result.won_lease());
        assert_eq!(result.value_state, ValueState::Missing);

        // Someone else's placeholder: pending.
        let result = parse_get(&Get::new("foo"), wire(b"VA 0 c7 Z", Some(b""))).unwrap();
        assert_eq!(result.status, GetStatus::Pending);
        assert!(result.lease_busy());
    }

    #[test]
    fn parse_get_stale_and_unchanged() {
        let operation = Get {
            lease_ttl: Some(30),
            ..Get::new("foo")
        };
        let result = parse_get(&operation, wire(b"VA 3 c7 X Z", Some(b"old"))).unwrap();
        assert_eq!(result.status, GetStatus::Hit);
        assert!(result.is_stale());
        assert_eq!(result.value.as_deref(), Some(&b"old"[..]));

        let operation = Get {
            unless_cas: Some(42),
            ..Get::new("foo")
        };
        let result = parse_get(&operation, wire(b"HD c42", None)).unwrap();
        assert_eq!(result.status, GetStatus::Unchanged);
        assert_eq!(result.value, None);
    }

    #[test]
    fn parse_mutation_statuses() {
        let set = Set::new("foo", "bar");
        assert_eq!(
            parse_set(&set, wire(b"HD", None)).unwrap().status,
            MutationStatus::Applied
        );
        assert_eq!(
            parse_set(&set, wire(b"EX", None)).unwrap().status,
            MutationStatus::CasMismatch
        );
        assert_eq!(
            parse_set(&set, wire(b"NS", None)).unwrap().status,
            MutationStatus::NotFound
        );
        let add = Set {
            mode: SetMode::Add,
            ..Set::new("foo", "bar")
        };
        assert_eq!(
            parse_set(&add, wire(b"NS", None)).unwrap().status,
            MutationStatus::AlreadyExists
        );
        assert_eq!(
            parse_delete(&Delete::new("foo"), wire(b"NF", None)).unwrap().status,
            MutationStatus::NotFound
        );
    }

    #[test]
    fn parse_arithmetic_value() {
        let operation = Arithmetic::new("counter");
        let result = parse_arithmetic(&operation, wire(b"VA 2 c9 t60", Some(b"42"))).unwrap();
        assert!(result.applied());
        assert_eq!(result.value, Some(42));
        assert_eq!(result.item.cas, Some(9));
        assert_eq!(result.item.ttl, Some(60));

        let result = parse_arithmetic(&operation, wire(b"NF", None)).unwrap();
        assert_eq!(result.status, MutationStatus::NotFound);
        assert_eq!(result.value, None);
    }
}

// ---------------------------------------------------------------------
// Scenario layer: plan functions build the wire command for one verb,
// finish functions reduce its response to the verb's answer. Both are pure,
// so the blocking and tokio clients share every semantic decision and only
// differ in how they move bytes.

pub(crate) mod scenario {
    use std::time::Duration;

    use super::super::error::{Error, Result};
    use super::super::meta_api::{
        ArithmeticMode, ArithmeticOptions, DeleteOptions, GetOptions, MetaCommandResult, SetMode, SetOptions,
        build_arithmetic, build_delete, build_get, build_set,
    };
    use super::super::meta_command::{MetaCommand, ReturnCode};
    use super::super::ttl::{Freshness, Ttl};
    use super::super::value::{Decode, Encode, Encoded};

    /// How long a miss-path lease lives: the zero-byte placeholder created
    /// by vivify expires on its own, so a crashed winner's exclusive right
    /// to recompute is re-elected later.
    pub(crate) const LEASE_TTL_SECS: u32 = 30;
    /// A cross-process loser's wait schedule; exhausted, it computes locally
    /// without writing back.
    pub(crate) const WAIT_BACKOFF: [Duration; 5] = [
        Duration::from_millis(25),
        Duration::from_millis(50),
        Duration::from_millis(100),
        Duration::from_millis(200),
        Duration::from_millis(400),
    ];
    /// Bound of the optimistic loops in `update` and `take`.
    pub(crate) const UPDATE_ATTEMPTS: usize = 8;

    /// Read-only item metadata returned by `inspect`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[non_exhaustive]
    pub struct ItemInfo {
        /// Time left before expiry; `None` when the item never expires.
        pub ttl: Option<Duration>,
        /// Stored value size in bytes.
        pub size: u64,
        /// Time since the item was last read or written.
        pub last_access: Duration,
        /// Whether the item was hit since it was stored.
        pub hit_before: bool,
    }

    /// One read response reduced to the facts the scenario layer branches
    /// on. A miss (`EN`) is `hit == false` with everything else empty.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub(crate) struct ReadView {
        pub(crate) hit: bool,
        pub(crate) value: Option<Vec<u8>>,
        pub(crate) flags: u32,
        pub(crate) cas: Option<u64>,
        pub(crate) ttl: Option<i64>,
        pub(crate) size: Option<u64>,
        pub(crate) last_access: Option<u64>,
        pub(crate) hit_before: Option<bool>,
        pub(crate) won: bool,
        pub(crate) busy: bool,
        pub(crate) stale: bool,
    }

    impl ReadView {
        /// A value-bearing hit, zero-byte values folded away: placeholders
        /// and genuinely empty items both read as "nothing here".
        fn has_value(&self) -> bool {
            self.hit && self.value.as_ref().is_some_and(|value| !value.is_empty())
        }

        /// The stale-recache token this read won by accident, to hand back
        /// with [`plan_return_win`]. memcached grants a stale item's single
        /// token to the first reader regardless of what it asked for and
        /// never re-grants it until the item is written; a read that cannot
        /// recompute would otherwise disable `fetch`'s election for the rest
        /// of the grace period.
        pub(crate) fn stale_win(&self) -> Option<StaleWin> {
            if self.stale && self.won {
                self.cas.map(|cas| StaleWin { cas, ttl: self.ttl })
            } else {
                None
            }
        }

        fn decode<T: Decode>(self) -> Result<T> {
            Ok(T::decode(self.value.unwrap_or_default(), self.flags)?)
        }
    }

    /// An accidentally consumed stale-recache token.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct StaleWin {
        pub(crate) cas: u64,
        ttl: Option<i64>,
    }

    pub(crate) fn read_view(wire: MetaCommandResult) -> Result<ReadView> {
        match wire.rc {
            ReturnCode::En => Ok(ReadView::default()),
            ReturnCode::Va | ReturnCode::Hd => Ok(ReadView {
                hit: true,
                value: wire.value,
                flags: wire.client_flags.unwrap_or(0),
                cas: wire.cas,
                ttl: wire.ttl,
                size: wire.size,
                last_access: wire.last_access,
                hit_before: wire.hit_before,
                won: wire.won,
                busy: wire.busy,
                stale: wire.stale,
            }),
            rc => Err(Error::Protocol(format!("unexpected read response {rc:?}"))),
        }
    }

    /// Every scenario-level read asks for the CAS and TTL so a stale win
    /// can be returned.
    fn base_read() -> GetOptions {
        GetOptions {
            return_client_flags: true,
            return_cas: true,
            return_ttl: true,
            ..GetOptions::default()
        }
    }

    /// The value read behind `update` and `take`.
    pub(crate) fn plan_probe(key: &[u8]) -> Result<MetaCommand> {
        build_get(key, &base_read())
    }

    /// A plain read, optionally sliding the expiry (`get_touch`).
    pub(crate) fn plan_get(key: &[u8], touch: Option<Ttl>) -> Result<MetaCommand> {
        let options = GetOptions {
            touch: touch.map(Ttl::wire).transpose()?,
            ..base_read()
        };
        build_get(key, &options)
    }

    /// Decode a read into the caller's type; a miss, a placeholder and a
    /// genuinely empty item all fold to `None` (the zero-byte rule).
    pub(crate) fn finish_get<T: Decode>(view: ReadView) -> Result<Option<T>> {
        if !view.has_value() {
            return Ok(None);
        }
        view.decode().map(Some)
    }

    /// Encode a value for a scenario-level write, enforcing the zero-byte
    /// rule: a zero-byte item is indistinguishable from a lease placeholder
    /// and every scenario read folds it to a miss.
    pub(crate) fn encode_value<V: Encode>(value: V) -> Result<Encoded> {
        let encoded = value.encode()?;
        if encoded.bytes.is_empty() {
            return Err(Error::EmptyValue);
        }
        Ok(encoded)
    }

    /// A store whose answer is whether it was applied; `NS` / `EX` / `NF`
    /// mean the write's condition failed, which is an answer rather than an
    /// error.
    pub(crate) fn plan_store(
        key: &[u8],
        value: &Encoded,
        ttl: Ttl,
        mode: SetMode,
        compare_cas: Option<u64>,
    ) -> Result<MetaCommand> {
        let options = SetOptions {
            client_flags: (value.flags != 0).then_some(value.flags),
            ttl: Some(ttl.wire()?),
            mode,
            compare_cas,
            ..SetOptions::default()
        };
        build_set(key, value.bytes.as_slice(), &options)
    }

    pub(crate) fn finish_store(wire: &MetaCommandResult) -> Result<bool> {
        match wire.rc {
            ReturnCode::Hd => Ok(true),
            ReturnCode::Ns | ReturnCode::Ex | ReturnCode::Nf => Ok(false),
            rc => Err(Error::Protocol(format!("unexpected store response {rc:?}"))),
        }
    }

    /// A delete, or a soft invalidation when `grace` is given, answering
    /// whether there was something to erase.
    pub(crate) fn plan_erase(key: &[u8], grace: Option<Ttl>, compare_cas: Option<u64>) -> Result<MetaCommand> {
        let options = DeleteOptions {
            compare_cas,
            invalidate: grace.is_some(),
            ttl: grace.map(Ttl::wire).transpose()?,
            ..DeleteOptions::default()
        };
        build_delete(key, &options)
    }

    pub(crate) fn finish_erase(wire: &MetaCommandResult) -> Result<bool> {
        match wire.rc {
            ReturnCode::Hd => Ok(true),
            ReturnCode::Nf | ReturnCode::Ex => Ok(false),
            rc => Err(Error::Protocol(format!("unexpected delete response {rc:?}"))),
        }
    }

    /// `invalidate`'s grace must be a positive duration; `Ttl::NEVER` would
    /// keep the item stale forever and zero is rejected by `Ttl` itself.
    pub(crate) fn grace_ttl(grace: Duration) -> Result<Ttl> {
        if grace.is_zero() {
            return Err(Error::Usage("invalidate grace must be positive; use delete for a hard delete"));
        }
        Ok(Ttl::from(grace))
    }

    /// A blind expiry extension without transferring the value.
    pub(crate) fn plan_touch(key: &[u8], ttl: Ttl) -> Result<MetaCommand> {
        let options = GetOptions {
            value: false,
            return_client_flags: false,
            touch: Some(ttl.wire()?),
            ..base_read()
        };
        build_get(key, &options)
    }

    /// `incr` / `decr`: a miss counts from zero, so the vivified item is
    /// seeded with the delta (or zero when decrementing toward the floor)
    /// and `ttl` rides the creation only.
    pub(crate) fn plan_counter(key: &[u8], delta: u64, decrement: bool, ttl: Ttl) -> Result<MetaCommand> {
        let options = ArithmeticOptions {
            delta: Some(delta),
            mode: if decrement {
                ArithmeticMode::Decrement
            } else {
                ArithmeticMode::Increment
            },
            initial: Some(if decrement { 0 } else { delta }),
            initial_ttl: Some(ttl.wire()?),
            return_value: true,
            ..ArithmeticOptions::default()
        };
        build_arithmetic(key, &options)
    }

    pub(crate) fn finish_counter(wire: &MetaCommandResult) -> Result<u64> {
        if wire.rc == ReturnCode::Va
            && let Some(value) = &wire.value
            && let Some(number) = std::str::from_utf8(value).ok().and_then(|text| text.parse::<u64>().ok())
        {
            return Ok(number);
        }
        Err(Error::Protocol(format!("counter did not return a value ({:?})", wire.rc)))
    }

    /// `append` / `prepend`: raw bytes, created on a miss with `ttl`.
    pub(crate) fn plan_concat(key: &[u8], fragment: &[u8], ttl: Ttl, prepend: bool) -> Result<MetaCommand> {
        if fragment.is_empty() {
            return Err(Error::Usage("append/prepend fragment must not be empty"));
        }
        let options = SetOptions {
            mode: if prepend { SetMode::Prepend } else { SetMode::Append },
            vivify_ttl: Some(ttl.wire()?),
            ..SetOptions::default()
        };
        build_set(key, fragment, &options)
    }

    pub(crate) fn finish_concat(wire: &MetaCommandResult) -> Result<()> {
        match wire.rc {
            ReturnCode::Hd => Ok(()),
            rc => Err(Error::Protocol(format!("unexpected append/prepend response {rc:?}"))),
        }
    }

    /// Metadata only, without bumping the LRU.
    pub(crate) fn plan_inspect(key: &[u8]) -> Result<MetaCommand> {
        let options = GetOptions {
            value: false,
            return_client_flags: false,
            return_size: true,
            return_last_access: true,
            return_hit_before: true,
            no_lru_bump: true,
            ..base_read()
        };
        build_get(key, &options)
    }

    pub(crate) fn finish_inspect(view: &ReadView) -> Option<ItemInfo> {
        if !view.hit {
            return None;
        }
        Some(ItemInfo {
            ttl: view
                .ttl
                .and_then(|ttl| u64::try_from(ttl).ok())
                .map(Duration::from_secs),
            size: view.size.unwrap_or(0),
            last_access: Duration::from_secs(view.last_access.unwrap_or(0)),
            hit_before: view.hit_before.unwrap_or(false),
        })
    }

    /// `fetch`'s combined read and server-side election: on a miss the
    /// vivify flag creates a placeholder and grants this reader the lease,
    /// near expiry the recache window elects one reader ahead of time, and
    /// during a soft-delete grace the stale marker elects one refresher.
    /// Returns the command with the resolved refresh window in seconds.
    pub(crate) fn plan_election(key: &[u8], freshness: Freshness) -> Result<(MetaCommand, Option<u32>)> {
        let refresh_before = freshness.refresh_before_secs()?;
        // Validate the ttl up front so a bad one is reported before any
        // lease is taken.
        freshness.ttl().wire()?;
        let options = GetOptions {
            vivify_ttl: Some(LEASE_TTL_SECS),
            recache_ttl: refresh_before,
            ..base_read()
        };
        Ok((build_get(key, &options)?, refresh_before))
    }

    /// What `fetch` does with an election read.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum FetchStep {
        /// A value-bearing hit with no election: return it.
        Serve { value: Vec<u8>, flags: u32 },
        /// A value-bearing hit whose refresh this reader won (refresh-ahead
        /// window or stale grace). The blocking client recomputes now; the
        /// tokio client serves the value and recomputes in the background.
        Refresh { value: Vec<u8>, flags: u32, cas: u64 },
        /// This reader owns the miss-path lease (or found a zero-byte item
        /// nobody is rewriting): run the loader, write back through `cas`.
        /// `release` says the placeholder must be deleted when nothing is
        /// written back, so the next reader re-elects immediately.
        Lead { cas: u64 },
        /// No coordination available (a miss despite vivify): compute
        /// without writing back.
        Local,
        /// Another process holds the lease: wait and re-read.
        Wait,
    }

    pub(crate) fn fetch_step(view: ReadView) -> FetchStep {
        if view.has_value() {
            let flags = view.flags;
            return match (view.won, view.cas) {
                (true, Some(cas)) => FetchStep::Refresh {
                    value: view.value.unwrap_or_default(),
                    flags,
                    cas,
                },
                _ => FetchStep::Serve {
                    value: view.value.unwrap_or_default(),
                    flags,
                },
            };
        }
        if !view.hit {
            return FetchStep::Local;
        }
        // A zero-byte item: the placeholder this read vivified and won, or
        // one nobody else is rewriting (a real empty item, or an empty item
        // whose stale token this read just won). Waiting on the latter
        // would pay the full backoff on every fetch forever.
        match view.cas {
            Some(cas) if view.won || !view.busy => FetchStep::Lead { cas },
            Some(_) => FetchStep::Wait,
            None => FetchStep::Local,
        }
    }

    /// The store that repays a lease: conditioned on the election's CAS so
    /// a delete, set or re-invalidation that landed meanwhile wins.
    pub(crate) fn plan_write_back(key: &[u8], value: &Encoded, ttl: Ttl, cas: u64) -> Result<MetaCommand> {
        plan_store(key, value, ttl, SetMode::Set, Some(cas))
    }

    /// Delete the placeholder whose lease could not be repaid.
    pub(crate) fn plan_release_lease(key: &[u8], cas: u64) -> Result<MetaCommand> {
        plan_erase(key, None, Some(cas))
    }

    /// Hand an accidentally consumed stale-recache token back: re-invalidate
    /// with the CAS just read, keeping the value and the remaining grace.
    pub(crate) fn plan_return_win(key: &[u8], win: StaleWin) -> Result<MetaCommand> {
        let options = DeleteOptions {
            compare_cas: Some(win.cas),
            invalidate: true,
            ttl: win
                .ttl
                .filter(|&ttl| ttl > 0)
                .map(|ttl| Ttl::secs(u32::try_from(ttl).unwrap_or(u32::MAX)).wire())
                .transpose()?,
            ..DeleteOptions::default()
        };
        build_delete(key, &options)
    }

    /// What `update` does with its probe read.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) struct UpdateStep {
        /// The current value to hand to the closure; `None` on a miss, a
        /// placeholder, or an item kept stale by `invalidate` (transforming
        /// invalidated data would launder it back to fresh).
        pub(crate) current: Option<(Vec<u8>, u32)>,
        /// The CAS to write through. Any existing item, stale or zero-byte
        /// included, can only be replaced by CAS; `None` means the key is
        /// absent and the write is an `add`.
        pub(crate) compare_cas: Option<u64>,
        /// A stale token to hand back when nothing gets written.
        pub(crate) stale_win: Option<StaleWin>,
    }

    pub(crate) fn update_step(view: ReadView) -> UpdateStep {
        let stale_win = view.stale_win();
        let compare_cas = if view.hit { view.cas } else { None };
        let current = if view.has_value() && !view.stale {
            Some((view.value.unwrap_or_default(), view.flags))
        } else {
            None
        };
        UpdateStep {
            current,
            compare_cas,
            stale_win,
        }
    }

    /// What `take` does with its probe read.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum TakeStep {
        /// Nothing to take (miss, placeholder, or stale item); hand back a
        /// stale token if one was won.
        Nothing(Option<StaleWin>),
        /// Decode this value and delete the item through `cas`.
        Take { value: Vec<u8>, flags: u32, cas: u64 },
    }

    pub(crate) fn take_step(view: ReadView) -> Result<TakeStep> {
        if !view.has_value() || view.stale {
            return Ok(TakeStep::Nothing(view.stale_win()));
        }
        let Some(cas) = view.cas else {
            return Err(Error::protocol("value read omitted the requested CAS"));
        };
        Ok(TakeStep::Take {
            value: view.value.unwrap_or_default(),
            flags: view.flags,
            cas,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::super::super::meta_api::parse_meta_result;
        use super::super::super::meta_command::MetaResponse;
        use super::*;

        fn view(header: &[u8], value: Option<&[u8]>) -> ReadView {
            let mut response = MetaResponse::parse_header(header).unwrap();
            response.value = value.map(|value| value.to_vec());
            read_view(parse_meta_result(response).unwrap()).unwrap()
        }

        fn wire(header: &[u8], value: Option<&[u8]>) -> MetaCommandResult {
            let mut response = MetaResponse::parse_header(header).unwrap();
            response.value = value.map(|value| value.to_vec());
            parse_meta_result(response).unwrap()
        }

        fn encoded(command: MetaCommand) -> String {
            String::from_utf8(command.encode().unwrap()).unwrap()
        }

        #[test]
        fn plans_encode_expected_commands() {
            assert_eq!(encoded(plan_probe(b"k").unwrap()), "mg k v f c t\r\n");
            assert_eq!(encoded(plan_get(b"k", None).unwrap()), "mg k v f c t\r\n");
            assert_eq!(
                encoded(plan_get(b"k", Some(Ttl::secs(60))).unwrap()),
                "mg k v f c t T60\r\n"
            );
            let value = Encoded::new("v", 16);
            assert_eq!(
                encoded(plan_store(b"k", &value, Ttl::secs(60), SetMode::Set, None).unwrap()),
                "ms k 1 F16 T60\r\nv\r\n"
            );
            assert_eq!(
                encoded(plan_store(b"k", &Encoded::new("v", 0), Ttl::NEVER, SetMode::Add, None).unwrap()),
                "ms k 1 ME T0\r\nv\r\n"
            );
            assert_eq!(
                encoded(plan_write_back(b"k", &value, Ttl::secs(5), 7).unwrap()),
                "ms k 1 F16 T5 C7\r\nv\r\n"
            );
            assert_eq!(encoded(plan_erase(b"k", None, None).unwrap()), "md k\r\n");
            assert_eq!(
                encoded(plan_erase(b"k", Some(Ttl::secs(30)), None).unwrap()),
                "md k I T30\r\n"
            );
            assert_eq!(encoded(plan_release_lease(b"k", 7).unwrap()), "md k C7\r\n");
            assert_eq!(encoded(plan_touch(b"k", Ttl::secs(60)).unwrap()), "mg k c t T60\r\n");
            assert_eq!(
                encoded(plan_counter(b"k", 2, false, Ttl::secs(60)).unwrap()),
                "ma k v D2 N60 J2\r\n"
            );
            assert_eq!(
                encoded(plan_counter(b"k", 2, true, Ttl::NEVER).unwrap()),
                "ma k MD v D2 N0 J0\r\n"
            );
            assert_eq!(
                encoded(plan_concat(b"k", b"x", Ttl::secs(60), false).unwrap()),
                "ms k 1 MA N60\r\nx\r\n"
            );
            assert_eq!(
                encoded(plan_concat(b"k", b"x", Ttl::secs(60), true).unwrap()),
                "ms k 1 MP N60\r\nx\r\n"
            );
            assert!(matches!(
                plan_concat(b"k", b"", Ttl::secs(60), false),
                Err(Error::Usage(_))
            ));
            assert_eq!(encoded(plan_inspect(b"k").unwrap()), "mg k c t s l h u\r\n");
        }

        #[test]
        fn election_plan() {
            let (command, window) = plan_election(b"k", Ttl::secs(300).into()).unwrap();
            assert_eq!(encoded(command), "mg k v f c t N30\r\n");
            assert_eq!(window, None);
            let (command, window) =
                plan_election(b"k", Ttl::secs(300).refresh_ahead(Duration::from_secs(30))).unwrap();
            assert_eq!(encoded(command), "mg k v f c t N30 R30\r\n");
            assert_eq!(window, Some(30));
            assert!(matches!(
                plan_election(b"k", Ttl::NEVER.refresh_ahead(Duration::from_secs(30))),
                Err(Error::Usage(_))
            ));
            assert!(matches!(plan_election(b"k", Ttl::secs(0).into()), Err(Error::Usage(_))));
        }

        #[test]
        fn return_win_plan() {
            let win = view(b"VA 3 c7 t40 X W", Some(b"old")).stale_win().unwrap();
            assert_eq!(encoded(plan_return_win(b"k", win).unwrap()), "md k I C7 T40\r\n");
            // An exhausted or unlimited remaining ttl sends no T.
            let win = view(b"VA 3 c7 t-1 X W", Some(b"old")).stale_win().unwrap();
            assert_eq!(encoded(plan_return_win(b"k", win).unwrap()), "md k I C7\r\n");
            assert!(view(b"VA 3 c7 t40 X Z", Some(b"old")).stale_win().is_none());
            assert!(view(b"VA 3 c7 t40 W", Some(b"old")).stale_win().is_none());
        }

        #[test]
        fn get_folds_zero_bytes_to_miss() {
            assert_eq!(finish_get::<String>(view(b"EN", None)).unwrap(), None);
            assert_eq!(finish_get::<String>(view(b"VA 0 c7 W", Some(b""))).unwrap(), None);
            assert_eq!(finish_get::<String>(view(b"VA 0 c7", Some(b""))).unwrap(), None);
            assert_eq!(
                finish_get::<String>(view(b"VA 3 f16 c7", Some(b"bar"))).unwrap(),
                Some("bar".to_string())
            );
            // Stale values are served by plain reads.
            assert_eq!(
                finish_get::<String>(view(b"VA 3 c7 X Z", Some(b"old"))).unwrap(),
                Some("old".to_string())
            );
            assert!(matches!(
                finish_get::<u64>(view(b"VA 1 c7", Some(b"x"))),
                Err(Error::Decode(_))
            ));
            assert!(matches!(read_view(wire(b"NS", None)), Err(Error::Protocol(_))));
        }

        #[test]
        fn empty_value_rule() {
            assert!(matches!(encode_value(""), Err(Error::EmptyValue)));
            assert!(matches!(encode_value(Vec::<u8>::new()), Err(Error::EmptyValue)));
            assert_eq!(encode_value("x").unwrap(), Encoded::new("x", 16));
        }

        #[test]
        fn store_and_erase_answers() {
            assert!(finish_store(&wire(b"HD", None)).unwrap());
            assert!(!finish_store(&wire(b"NS", None)).unwrap());
            assert!(!finish_store(&wire(b"EX", None)).unwrap());
            assert!(!finish_store(&wire(b"NF", None)).unwrap());
            assert!(finish_store(&wire(b"EN", None)).is_err());
            assert!(finish_erase(&wire(b"HD", None)).unwrap());
            assert!(!finish_erase(&wire(b"NF", None)).unwrap());
            assert!(!finish_erase(&wire(b"EX", None)).unwrap());
            assert!(finish_erase(&wire(b"VA 0", Some(b""))).is_err());
            assert_eq!(finish_counter(&wire(b"VA 2", Some(b"42"))).unwrap(), 42);
            assert!(finish_counter(&wire(b"NF", None)).is_err());
            assert!(finish_counter(&wire(b"VA 1", Some(b"x"))).is_err());
            assert!(finish_concat(&wire(b"HD", None)).is_ok());
            assert!(finish_concat(&wire(b"NS", None)).is_err());
            assert!(matches!(grace_ttl(Duration::ZERO), Err(Error::Usage(_))));
            assert_eq!(grace_ttl(Duration::from_secs(60)).unwrap(), Ttl::secs(60));
        }

        #[test]
        fn inspect_answer() {
            assert_eq!(finish_inspect(&view(b"EN", None)), None);
            let info = finish_inspect(&view(b"HD c1 t60 s3 l5 h1", None)).unwrap();
            assert_eq!(info.ttl, Some(Duration::from_secs(60)));
            assert_eq!(info.size, 3);
            assert_eq!(info.last_access, Duration::from_secs(5));
            assert!(info.hit_before);
            let info = finish_inspect(&view(b"HD c1 t-1 s3 l0 h0", None)).unwrap();
            assert_eq!(info.ttl, None);
            assert!(!info.hit_before);
        }

        #[test]
        fn fetch_state_machine() {
            let serve = |header: &[u8], value: &[u8]| fetch_step(view(header, Some(value)));
            // Fresh hit.
            assert_eq!(
                serve(b"VA 3 f16 c7 t10", b"bar"),
                FetchStep::Serve {
                    value: b"bar".to_vec(),
                    flags: 16
                }
            );
            // Refresh-ahead win and stale-grace win: this reader refreshes.
            assert_eq!(
                serve(b"VA 3 c7 t10 W", b"bar"),
                FetchStep::Refresh {
                    value: b"bar".to_vec(),
                    flags: 0,
                    cas: 7
                }
            );
            assert!(matches!(serve(b"VA 3 c7 t10 X W", b"old"), FetchStep::Refresh { cas: 7, .. }));
            // Stale value while another reader refreshes: keep serving it.
            assert!(matches!(serve(b"VA 3 c7 t10 X Z", b"old"), FetchStep::Serve { .. }));
            // Our own vivified placeholder: lead.
            assert_eq!(serve(b"VA 0 c7 W", b""), FetchStep::Lead { cas: 7 });
            // Someone else's placeholder: wait.
            assert_eq!(serve(b"VA 0 c7 Z", b""), FetchStep::Wait);
            // A real zero-byte item nobody is rewriting: lead (fifth branch).
            assert_eq!(serve(b"VA 0 c7", b""), FetchStep::Lead { cas: 7 });
            // A miss despite vivify: no coordination.
            assert_eq!(fetch_step(view(b"EN", None)), FetchStep::Local);
        }

        #[test]
        fn update_state_machine() {
            let step = update_step(view(b"VA 3 f16 c7 t10", Some(b"bar")));
            assert_eq!(step.current, Some((b"bar".to_vec(), 16)));
            assert_eq!(step.compare_cas, Some(7));
            assert_eq!(step.stale_win, None);

            let step = update_step(view(b"EN", None));
            assert_eq!(step.current, None);
            assert_eq!(step.compare_cas, None);

            // Stale reads as a miss but the item exists: write by CAS and
            // remember the token to hand back.
            let step = update_step(view(b"VA 3 c7 t10 X W", Some(b"old")));
            assert_eq!(step.current, None);
            assert_eq!(step.compare_cas, Some(7));
            assert_eq!(step.stale_win.map(|win| win.cas), Some(7));

            // A placeholder has a CAS; an add would fail until it expires.
            let step = update_step(view(b"VA 0 c7 Z", Some(b"")));
            assert_eq!(step.current, None);
            assert_eq!(step.compare_cas, Some(7));
        }

        #[test]
        fn take_state_machine() {
            assert_eq!(take_step(view(b"EN", None)).unwrap(), TakeStep::Nothing(None));
            assert_eq!(take_step(view(b"VA 0 c7 W", Some(b""))).unwrap(), TakeStep::Nothing(None));
            assert!(matches!(
                take_step(view(b"VA 3 c7 t10 X W", Some(b"old"))).unwrap(),
                TakeStep::Nothing(Some(_))
            ));
            assert_eq!(
                take_step(view(b"VA 3 f16 c7", Some(b"bar"))).unwrap(),
                TakeStep::Take {
                    value: b"bar".to_vec(),
                    flags: 16,
                    cas: 7
                }
            );
            assert!(take_step(view(b"VA 3 f16", Some(b"bar"))).is_err());
        }
    }
}
