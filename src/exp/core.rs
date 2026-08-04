//! Transport-independent semantic core: turns operations into wire commands
//! and pairs wire results back with their operations to produce typed
//! results. Both clients (blocking and tokio) are thin I/O loops around
//! these functions, and the future pipeline executor will reuse them.

use std::borrow::Cow;

use crate::error::{ClientError, MemcacheError, ServerError};

use super::meta_api::{
    ArithmeticOptions, DeleteOptions, GetOptions, MetaCommandResult, SetMode, SetOptions, build_arithmetic,
    build_delete, build_get, build_set,
};
use super::meta_command::{MetaCommand, ReturnCode};
use super::operation::{Arithmetic, Delete, Get, Op, Set};
use super::result::{
    ArithmeticResult, GetResult, GetStatus, ItemMeta, LeaseState, MutationResult, MutationStatus, OpResult, ValueState,
};

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
    fn prepare(&self) -> Result<MetaCommand, MemcacheError>;

    #[doc(hidden)]
    fn parse(&self, wire: MetaCommandResult) -> Result<Self::Output, MemcacheError>;
}

impl Operation for Get {
    type Output = GetResult;

    fn key(&self) -> &[u8] {
        &self.key
    }

    fn prepare(&self) -> Result<MetaCommand, MemcacheError> {
        prepare_get(self)
    }

    fn parse(&self, wire: MetaCommandResult) -> Result<GetResult, MemcacheError> {
        parse_get(self, wire)
    }
}

impl Operation for Set {
    type Output = MutationResult;

    fn key(&self) -> &[u8] {
        &self.key
    }

    fn prepare(&self) -> Result<MetaCommand, MemcacheError> {
        prepare_set(self)
    }

    fn parse(&self, wire: MetaCommandResult) -> Result<MutationResult, MemcacheError> {
        parse_set(self, wire)
    }
}

impl Operation for Delete {
    type Output = MutationResult;

    fn key(&self) -> &[u8] {
        &self.key
    }

    fn prepare(&self) -> Result<MetaCommand, MemcacheError> {
        prepare_delete(self)
    }

    fn parse(&self, wire: MetaCommandResult) -> Result<MutationResult, MemcacheError> {
        parse_delete(self, wire)
    }
}

impl Operation for Arithmetic {
    type Output = ArithmeticResult;

    fn key(&self) -> &[u8] {
        &self.key
    }

    fn prepare(&self) -> Result<MetaCommand, MemcacheError> {
        prepare_arithmetic(self)
    }

    fn parse(&self, wire: MetaCommandResult) -> Result<ArithmeticResult, MemcacheError> {
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

    fn prepare(&self) -> Result<MetaCommand, MemcacheError> {
        match self {
            Op::Get(operation) => operation.prepare(),
            Op::Set(operation) => operation.prepare(),
            Op::Delete(operation) => operation.prepare(),
            Op::Arithmetic(operation) => operation.prepare(),
        }
    }

    fn parse(&self, wire: MetaCommandResult) -> Result<OpResult, MemcacheError> {
        match self {
            Op::Get(operation) => operation.parse(wire).map(OpResult::Get),
            Op::Set(operation) => operation.parse(wire).map(OpResult::Mutation),
            Op::Delete(operation) => operation.parse(wire).map(OpResult::Mutation),
            Op::Arithmetic(operation) => operation.parse(wire).map(OpResult::Arithmetic),
        }
    }
}

fn invalid<T>(message: &'static str) -> Result<T, MemcacheError> {
    Err(ClientError::Error(Cow::Borrowed(message)).into())
}

/// Best-effort duplicate of an error, so a failure that covers a whole batch
/// group can be reported on each of the group's operations. io errors keep
/// their kind and message but lose the source chain; variants that cannot
/// arise from meta connections fall back to their rendered message.
pub(crate) fn duplicate_error(error: &MemcacheError) -> MemcacheError {
    match error {
        MemcacheError::IOError(err) => MemcacheError::IOError(std::io::Error::new(err.kind(), err.to_string())),
        MemcacheError::ClientError(err) => err.clone().into(),
        MemcacheError::ServerError(err) => err.clone().into(),
        MemcacheError::CommandError(err) => err.clone().into(),
        MemcacheError::ParseError(err) => err.clone().into(),
        other => ClientError::Error(Cow::Owned(other.to_string())).into(),
    }
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
) -> Result<BatchPlan, MemcacheError> {
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

fn unexpected<T>(message: &'static str) -> Result<T, MemcacheError> {
    Err(ServerError::BadResponse(Cow::Borrowed(message)).into())
}

pub(crate) fn prepare_get(operation: &Get) -> Result<MetaCommand, MemcacheError> {
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
        touch: operation.touch,
        vivify_ttl: operation.lease_ttl,
        recache_ttl: operation.refresh_before,
        unless_cas: operation.unless_cas,
        ..GetOptions::default()
    };
    build_get(operation.key.clone(), &options)
}

pub(crate) fn prepare_set(operation: &Set) -> Result<MetaCommand, MemcacheError> {
    if operation.vivify_ttl == Some(0) {
        return invalid("vivify_ttl must be >= 1");
    }
    let options = SetOptions {
        // An omitted F stores flags 0, so only non-zero flags go on the wire.
        client_flags: (operation.client_flags != 0).then_some(operation.client_flags),
        ttl: operation.ttl,
        mode: operation.mode,
        compare_cas: operation.compare_cas,
        new_cas: operation.version,
        vivify_ttl: operation.vivify_ttl,
        return_cas: operation.return_cas,
        ..SetOptions::default()
    };
    build_set(operation.key.clone(), operation.value.clone(), &options)
}

pub(crate) fn prepare_delete(operation: &Delete) -> Result<MetaCommand, MemcacheError> {
    if operation.stale_for.is_some() && !operation.invalidate {
        return invalid("stale_for is only valid for invalidate");
    }
    let options = DeleteOptions {
        compare_cas: operation.compare_cas,
        invalidate: operation.invalidate,
        ttl: operation.stale_for,
        ..DeleteOptions::default()
    };
    build_delete(operation.key.clone(), &options)
}

pub(crate) fn prepare_arithmetic(operation: &Arithmetic) -> Result<MetaCommand, MemcacheError> {
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
        ttl: operation.ttl,
        compare_cas: operation.compare_cas,
        new_cas: operation.version,
        return_value: true,
        return_ttl: operation.return_ttl,
        return_cas: operation.return_cas,
        ..ArithmeticOptions::default()
    };
    build_arithmetic(operation.key.clone(), &options)
}

fn mutation_status(is_add: bool, rc: ReturnCode) -> Result<MutationStatus, MemcacheError> {
    match rc {
        ReturnCode::Hd | ReturnCode::Va => Ok(MutationStatus::Stored),
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

pub(crate) fn parse_get(operation: &Get, wire: MetaCommandResult) -> Result<GetResult, MemcacheError> {
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

pub(crate) fn parse_set(operation: &Set, wire: MetaCommandResult) -> Result<MutationResult, MemcacheError> {
    Ok(MutationResult {
        key: operation.key.clone(),
        status: mutation_status(operation.mode == SetMode::Add, wire.rc)?,
        cas: wire.cas,
    })
}

pub(crate) fn parse_delete(operation: &Delete, wire: MetaCommandResult) -> Result<MutationResult, MemcacheError> {
    Ok(MutationResult {
        key: operation.key.clone(),
        status: mutation_status(false, wire.rc)?,
        cas: wire.cas,
    })
}

pub(crate) fn parse_arithmetic(
    operation: &Arithmetic,
    wire: MetaCommandResult,
) -> Result<ArithmeticResult, MemcacheError> {
    let status = mutation_status(false, wire.rc)?;
    let value = match (&wire.value, wire.rc) {
        (Some(value), ReturnCode::Va) if !value.is_empty() => Some(std::str::from_utf8(value)?.parse::<u64>()?),
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
            touch: Some(60),
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
        // A &str value carries FLAG_STR (16) from ToValue.
        let command = prepare_set(&Set::new("foo", "bar")).unwrap();
        assert_eq!(command.encode().unwrap(), b"ms foo 3 F16\r\nbar\r\n".to_vec());

        let command = prepare_set(&Set::new("foo", b"bar")).unwrap();
        assert_eq!(command.encode().unwrap(), b"ms foo 3\r\nbar\r\n".to_vec());

        let operation = Set {
            mode: SetMode::Add,
            ttl: Some(60),
            return_cas: true,
            ..Set::new("foo", "bar")
        };
        let command = prepare_set(&operation).unwrap();
        assert_eq!(command.encode().unwrap(), b"ms foo 3 ME c F16 T60\r\nbar\r\n".to_vec());

        // Non-zero client flags from ToValue go on the wire as F.
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
                stale_for: Some(30),
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
            MutationStatus::Stored
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
        assert!(result.stored());
        assert_eq!(result.value, Some(42));
        assert_eq!(result.item.cas, Some(9));
        assert_eq!(result.item.ttl, Some(60));

        let result = parse_arithmetic(&operation, wire(b"NF", None)).unwrap();
        assert_eq!(result.status, MutationStatus::NotFound);
        assert_eq!(result.value, None);
    }
}
