//! Typed 1:1 mapping of the memcached meta protocol.
//!
//! This module builds wire commands (`mg`/`ms`/`md`/`ma`/`mn`/`me`) from
//! option structs and parses wire responses into [`MetaCommandResult`]. Every
//! option field corresponds to exactly one protocol flag. This layer performs
//! no serialization and no semantic interpretation of results; both belong to
//! the high-level client. Validation is limited to argument ranges and
//! combinations the protocol silently ignores.
//!
//! The option structs are `#[non_exhaustive]`: start from `Default` and set
//! the fields you need.
//!
//! The `q` (noreply) and `b` (base64 key) flags are intentionally not
//! exposed: quiet semantics are an internal concern of the pipeline executor,
//! and binary keys are base64-encoded automatically by
//! [`MetaCommand`](super::MetaCommand).

use std::borrow::Cow;
use std::collections::HashMap;

use crate::error::{ClientError, MemcacheError};

use super::meta_command::{MetaCommand, MetaOp, MetaResponse, ReturnCode, base64_decode, encode_key};

const OPAQUE_MAX: usize = 32;

fn invalid<T>(message: &'static str) -> Result<T, MemcacheError> {
    Err(ClientError::Error(Cow::Borrowed(message)).into())
}

fn opaque_flag(opaque: &[u8]) -> Result<Vec<u8>, MemcacheError> {
    if opaque.is_empty() || opaque.len() > OPAQUE_MAX {
        return invalid("opaque must be 1-32 bytes");
    }
    if opaque.iter().any(|&byte| byte <= 0x20 || byte == 0x7f) {
        return invalid("opaque must not contain whitespace or control bytes");
    }
    let mut flag = Vec::with_capacity(opaque.len() + 1);
    flag.push(b'O');
    flag.extend_from_slice(opaque);
    Ok(flag)
}

struct FlagBuilder(Vec<Vec<u8>>);

impl FlagBuilder {
    fn new() -> FlagBuilder {
        FlagBuilder(Vec::new())
    }

    fn marker(&mut self, enabled: bool, wire: &'static [u8]) {
        if enabled {
            self.0.push(wire.to_vec());
        }
    }

    fn token(&mut self, prefix: u8, value: Option<u64>) {
        if let Some(value) = value {
            let mut flag = vec![prefix];
            flag.extend_from_slice(value.to_string().as_bytes());
            self.0.push(flag);
        }
    }

    fn opaque(&mut self, opaque: Option<&[u8]>) -> Result<(), MemcacheError> {
        if let Some(opaque) = opaque {
            self.0.push(opaque_flag(opaque)?);
        }
        Ok(())
    }

    fn into_inner(self) -> Vec<Vec<u8>> {
        self.0
    }
}

/// Options for [`build_get`] (`mg`). Each field maps to one protocol flag.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct GetOptions {
    /// `v` - return the item value.
    pub value: bool,
    /// `f` - return the client flags.
    pub return_client_flags: bool,
    /// `c` - return the item CAS.
    pub return_cas: bool,
    /// `t` - return the remaining TTL.
    pub return_ttl: bool,
    /// `s` - return the stored size.
    pub return_size: bool,
    /// `l` - return seconds since last access.
    pub return_last_access: bool,
    /// `h` - return whether the item was hit before.
    pub return_hit_before: bool,
    /// `k` - echo the key in the response.
    pub return_key: bool,
    /// `u` - don't bump the item in the LRU.
    pub no_lru_bump: bool,
    /// `T<ttl>` - update the item TTL.
    pub touch: Option<u32>,
    /// `N<ttl>` - vivify a missing item with this TTL and grant a lease.
    pub vivify_ttl: Option<u32>,
    /// `R<ttl>` - grant a recache lease when the remaining TTL is below this.
    pub recache_ttl: Option<u32>,
    /// `C<cas>` - suppress the value when the item CAS still matches.
    pub unless_cas: Option<u64>,
    /// `E<cas>` - replace the item CAS with this value.
    pub new_cas: Option<u64>,
    /// `O<token>` - opaque token echoed back in the response.
    pub opaque: Option<Vec<u8>>,
}

impl Default for GetOptions {
    fn default() -> GetOptions {
        GetOptions {
            value: true,
            return_client_flags: false,
            return_cas: false,
            return_ttl: false,
            return_size: false,
            return_last_access: false,
            return_hit_before: false,
            return_key: false,
            no_lru_bump: false,
            touch: None,
            vivify_ttl: None,
            recache_ttl: None,
            unless_cas: None,
            new_cas: None,
            opaque: None,
        }
    }
}

pub fn build_get(key: impl Into<Vec<u8>>, options: &GetOptions) -> Result<MetaCommand, MemcacheError> {
    if options.recache_ttl == Some(0) {
        return invalid("recache_ttl must be >= 1");
    }
    let mut flags = FlagBuilder::new();
    flags.marker(options.value, b"v");
    flags.marker(options.return_client_flags, b"f");
    flags.marker(options.return_cas, b"c");
    flags.marker(options.return_ttl, b"t");
    flags.marker(options.return_size, b"s");
    flags.marker(options.return_last_access, b"l");
    flags.marker(options.return_hit_before, b"h");
    flags.marker(options.return_key, b"k");
    flags.marker(options.no_lru_bump, b"u");
    flags.token(b'T', options.touch.map(u64::from));
    flags.token(b'N', options.vivify_ttl.map(u64::from));
    flags.token(b'R', options.recache_ttl.map(u64::from));
    flags.token(b'C', options.unless_cas);
    flags.token(b'E', options.new_cas);
    flags.opaque(options.opaque.as_deref())?;
    let mut command = MetaCommand::new(MetaOp::Get, key);
    command.flags = flags.into_inner();
    Ok(command)
}

/// Storage mode for [`build_set`] (`ms` `M` flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum SetMode {
    /// Unconditional store (the protocol default, no mode flag).
    #[default]
    Set,
    /// `ME` - store only when the item does not exist.
    Add,
    /// `MR` - store only when the item exists.
    Replace,
    /// `MA` - append raw bytes to the stored value.
    Append,
    /// `MP` - prepend raw bytes to the stored value.
    Prepend,
}

impl SetMode {
    fn flag(self) -> Option<&'static [u8]> {
        match self {
            SetMode::Set => None,
            SetMode::Add => Some(b"ME"),
            SetMode::Replace => Some(b"MR"),
            SetMode::Append => Some(b"MA"),
            SetMode::Prepend => Some(b"MP"),
        }
    }
}

/// Options for [`build_set`] (`ms`). Each field maps to one protocol flag.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct SetOptions {
    /// `F<flags>` - client flags stored with the item.
    pub client_flags: Option<u32>,
    /// `T<ttl>` - item TTL.
    pub ttl: Option<u32>,
    /// `M<mode>` - storage mode.
    pub mode: SetMode,
    /// `C<cas>` - store only when the item CAS matches.
    pub compare_cas: Option<u64>,
    /// `E<cas>` - replace the item CAS with this value.
    pub new_cas: Option<u64>,
    /// `I` - invalidate: mark the item stale instead of replacing it.
    pub invalidate: bool,
    /// `N<ttl>` - for append/prepend, vivify a missing item with this TTL.
    pub vivify_ttl: Option<u32>,
    /// `c` - return the new item CAS.
    pub return_cas: bool,
    /// `s` - return the stored size.
    pub return_size: bool,
    /// `k` - echo the key in the response.
    pub return_key: bool,
    /// `O<token>` - opaque token echoed back in the response.
    pub opaque: Option<Vec<u8>>,
}

pub fn build_set(
    key: impl Into<Vec<u8>>,
    value: impl Into<Vec<u8>>,
    options: &SetOptions,
) -> Result<MetaCommand, MemcacheError> {
    let concatenation = matches!(options.mode, SetMode::Append | SetMode::Prepend);
    if options.vivify_ttl.is_some() && !concatenation {
        return invalid("vivify_ttl is only valid for append/prepend");
    }
    if options.ttl.is_some() && concatenation {
        // The server ignores T for concatenation; the miss path takes its
        // TTL from N (vivify_ttl) instead. Reject the no-op.
        return invalid("ttl is ignored for append/prepend; use vivify_ttl");
    }
    if options.compare_cas.is_some() && options.mode == SetMode::Add {
        // Add only stores when no item exists, so there is no CAS to
        // compare; the protocol leaves the combination undefined.
        return invalid("compare_cas cannot be combined with add mode");
    }
    let mut flags = FlagBuilder::new();
    if let Some(mode_flag) = options.mode.flag() {
        flags.marker(true, mode_flag);
    }
    flags.marker(options.invalidate, b"I");
    flags.marker(options.return_cas, b"c");
    flags.marker(options.return_size, b"s");
    flags.marker(options.return_key, b"k");
    flags.token(b'F', options.client_flags.map(u64::from));
    flags.token(b'T', options.ttl.map(u64::from));
    flags.token(b'C', options.compare_cas);
    flags.token(b'E', options.new_cas);
    flags.token(b'N', options.vivify_ttl.map(u64::from));
    flags.opaque(options.opaque.as_deref())?;
    let mut command = MetaCommand::new(MetaOp::Set, key);
    command.flags = flags.into_inner();
    command.value = Some(value.into());
    Ok(command)
}

/// Options for [`build_delete`] (`md`). Each field maps to one protocol flag.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct DeleteOptions {
    /// `C<cas>` - delete only when the item CAS matches.
    pub compare_cas: Option<u64>,
    /// `E<cas>` - for invalidate, replace the item CAS with this value.
    pub new_cas: Option<u64>,
    /// `I` - invalidate: mark the item stale instead of removing it.
    pub invalidate: bool,
    /// `T<ttl>` - for invalidate, the TTL the stale item keeps.
    pub ttl: Option<u32>,
    /// `x` - drop the value but keep the item.
    pub drop_value: bool,
    /// `k` - echo the key in the response.
    pub return_key: bool,
    /// `O<token>` - opaque token echoed back in the response.
    pub opaque: Option<Vec<u8>>,
}

pub fn build_delete(key: impl Into<Vec<u8>>, options: &DeleteOptions) -> Result<MetaCommand, MemcacheError> {
    if options.ttl.is_some() && !options.invalidate {
        // The server only applies T when paired with I; reject the no-op.
        return invalid("ttl is only applied when invalidate is set");
    }
    let mut flags = FlagBuilder::new();
    flags.marker(options.invalidate, b"I");
    flags.marker(options.drop_value, b"x");
    flags.marker(options.return_key, b"k");
    flags.token(b'C', options.compare_cas);
    flags.token(b'E', options.new_cas);
    flags.token(b'T', options.ttl.map(u64::from));
    flags.opaque(options.opaque.as_deref())?;
    let mut command = MetaCommand::new(MetaOp::Delete, key);
    command.flags = flags.into_inner();
    Ok(command)
}

/// Arithmetic direction for [`build_arithmetic`] (`ma` `M` flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ArithmeticMode {
    /// Increment (the protocol default, no mode flag).
    #[default]
    Increment,
    /// `MD` - decrement.
    Decrement,
}

/// Options for [`build_arithmetic`] (`ma`). Each field maps to one protocol
/// flag.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ArithmeticOptions {
    /// `D<delta>` - the delta to apply (the server defaults to 1).
    pub delta: Option<u64>,
    /// `M<mode>` - increment or decrement.
    pub mode: ArithmeticMode,
    /// `J<value>` - initial value when vivifying a missing item.
    pub initial: Option<u64>,
    /// `N<ttl>` - vivify a missing item with this TTL.
    pub initial_ttl: Option<u32>,
    /// `T<ttl>` - update the item TTL.
    pub ttl: Option<u32>,
    /// `C<cas>` - apply only when the item CAS matches.
    pub compare_cas: Option<u64>,
    /// `E<cas>` - replace the item CAS with this value.
    pub new_cas: Option<u64>,
    /// `v` - return the new value.
    pub return_value: bool,
    /// `t` - return the remaining TTL.
    pub return_ttl: bool,
    /// `c` - return the new item CAS.
    pub return_cas: bool,
    /// `k` - echo the key in the response.
    pub return_key: bool,
    /// `O<token>` - opaque token echoed back in the response.
    pub opaque: Option<Vec<u8>>,
}

impl Default for ArithmeticOptions {
    fn default() -> ArithmeticOptions {
        ArithmeticOptions {
            delta: None,
            mode: ArithmeticMode::Increment,
            initial: None,
            initial_ttl: None,
            ttl: None,
            compare_cas: None,
            new_cas: None,
            return_value: true,
            return_ttl: false,
            return_cas: false,
            return_key: false,
            opaque: None,
        }
    }
}

pub fn build_arithmetic(key: impl Into<Vec<u8>>, options: &ArithmeticOptions) -> Result<MetaCommand, MemcacheError> {
    if options.initial.is_some() && options.initial_ttl.is_none() {
        // J is silently ignored without N; reject the no-op.
        return invalid("initial requires initial_ttl to vivify on miss");
    }
    let mut flags = FlagBuilder::new();
    flags.marker(options.mode == ArithmeticMode::Decrement, b"MD");
    flags.marker(options.return_value, b"v");
    flags.marker(options.return_ttl, b"t");
    flags.marker(options.return_cas, b"c");
    flags.marker(options.return_key, b"k");
    flags.token(b'D', options.delta);
    flags.token(b'N', options.initial_ttl.map(u64::from));
    flags.token(b'J', options.initial);
    flags.token(b'T', options.ttl.map(u64::from));
    flags.token(b'C', options.compare_cas);
    flags.token(b'E', options.new_cas);
    flags.opaque(options.opaque.as_deref())?;
    let mut command = MetaCommand::new(MetaOp::Arithmetic, key);
    command.flags = flags.into_inner();
    Ok(command)
}

/// Build an `mn` no-op, used as a pipeline barrier.
pub fn build_noop() -> MetaCommand {
    MetaCommand::new(MetaOp::Noop, Vec::new())
}

/// Build an `me` debug command.
pub fn build_debug(key: impl Into<Vec<u8>>) -> Result<MetaCommand, MemcacheError> {
    let key = key.into();
    let (_, needs_base64) = encode_key(&key)?;
    if needs_base64 {
        // protocol.txt documents the `b` flag for `me`, but real servers
        // (verified on memcached 1.6.45) look up the base64 token literally,
        // so a binary-key debug would silently report a miss for a live item.
        return invalid("meta debug does not support keys that require base64");
    }
    Ok(MetaCommand::new(MetaOp::Debug, key))
}

/// A fully parsed meta protocol response.
///
/// The typed fields are response flags decoded to native types; a field is
/// `None` (or `false` for markers) when the server did not send the flag.
/// [`raw_flags`](Self::raw_flags) keeps the raw tokens for anything this
/// layer does not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MetaCommandResult {
    /// The wire return code.
    pub rc: ReturnCode,
    /// The raw data block, if any.
    pub value: Option<Vec<u8>>,
    /// `c` - item CAS.
    pub cas: Option<u64>,
    /// `t` - remaining TTL (`-1` means unlimited).
    pub ttl: Option<i64>,
    /// `f` - client flags.
    pub client_flags: Option<u32>,
    /// `s` - stored size.
    pub size: Option<u64>,
    /// `l` - seconds since last access.
    pub last_access: Option<u64>,
    /// `h` - whether the item was hit before.
    pub hit_before: Option<bool>,
    /// `k` - the echoed key (base64-decoded when the `b` flag is present).
    pub key: Option<Vec<u8>>,
    /// `O` - the echoed opaque token.
    pub opaque: Option<Vec<u8>>,
    /// `W` - this client won a vivify/recache lease.
    pub won: bool,
    /// `Z` - another client holds the lease.
    pub busy: bool,
    /// `X` - the value is stale.
    pub stale: bool,
    pub(crate) flags: Vec<Vec<u8>>,
}

impl MetaCommandResult {
    /// Whether the command succeeded (`HD` or `VA`).
    pub fn ok(&self) -> bool {
        matches!(self.rc, ReturnCode::Hd | ReturnCode::Va)
    }

    /// The raw response flag tokens, including any this layer does not
    /// decode.
    pub fn raw_flags(&self) -> impl Iterator<Item = &[u8]> {
        self.flags.iter().map(Vec::as_slice)
    }
}

fn parse_int<T: std::str::FromStr<Err = std::num::ParseIntError>>(token: &[u8]) -> Result<T, MemcacheError> {
    Ok(std::str::from_utf8(token)?.parse::<T>()?)
}

/// Decode the response flags of a [`MetaResponse`] into a
/// [`MetaCommandResult`]. Unknown flags are kept in `flags` but otherwise
/// ignored.
pub fn parse_meta_result(response: MetaResponse) -> Result<MetaCommandResult, MemcacheError> {
    let mut result = MetaCommandResult {
        rc: response.rc,
        value: response.value,
        cas: None,
        ttl: None,
        client_flags: None,
        size: None,
        last_access: None,
        hit_before: None,
        key: None,
        opaque: None,
        won: false,
        busy: false,
        stale: false,
        flags: response.flags,
    };
    let mut key_base64 = false;
    for flag in &result.flags {
        let Some((&code, token)) = flag.split_first() else {
            continue;
        };
        match code {
            b'f' => result.client_flags = Some(parse_int(token)?),
            b'c' => result.cas = Some(parse_int(token)?),
            b't' => result.ttl = Some(parse_int(token)?),
            b'l' => result.last_access = Some(parse_int(token)?),
            b's' => result.size = Some(parse_int(token)?),
            b'h' => result.hit_before = Some(token != b"0"),
            b'k' => result.key = Some(token.to_vec()),
            b'O' => result.opaque = Some(token.to_vec()),
            b'W' => result.won = true,
            b'Z' => result.busy = true,
            b'X' => result.stale = true,
            b'b' => key_base64 = true,
            _ => {}
        }
    }
    if key_base64 && let Some(key) = &result.key {
        result.key = Some(base64_decode(key)?);
    }
    Ok(result)
}

/// Parse an `me` response into its `name=value` fields. Returns `None` on a
/// miss (`EN`).
pub fn parse_debug_result(response: &MetaResponse) -> Result<Option<HashMap<String, String>>, MemcacheError> {
    if response.rc == ReturnCode::En {
        return Ok(None);
    }
    if response.rc != ReturnCode::Me {
        return Err(crate::error::ServerError::BadResponse(Cow::Owned(format!(
            "unexpected debug response {:?}",
            response.rc
        )))
        .into());
    }
    let mut fields = HashMap::new();
    // The first token is the (possibly base64) key; the rest are name=value.
    for token in response.flags.iter().skip(1) {
        let mut split = token.splitn(2, |&byte| byte == b'=');
        let name = split.next().unwrap_or_default();
        let value = split.next().unwrap_or_default();
        fields.insert(String::from_utf8(name.to_vec())?, String::from_utf8(value.to_vec())?);
    }
    Ok(Some(fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_get_default() {
        let command = build_get("foo", &GetOptions::default()).unwrap();
        assert_eq!(command.encode().unwrap(), b"mg foo v\r\n".to_vec());
    }

    #[test]
    fn build_get_all_flags() {
        let options = GetOptions {
            value: true,
            return_client_flags: true,
            return_cas: true,
            return_ttl: true,
            return_size: true,
            return_last_access: true,
            return_hit_before: true,
            return_key: true,
            no_lru_bump: true,
            touch: Some(60),
            vivify_ttl: Some(30),
            recache_ttl: Some(10),
            unless_cas: Some(7),
            new_cas: Some(8),
            opaque: Some(b"tok".to_vec()),
        };
        let command = build_get("foo", &options).unwrap();
        assert_eq!(
            command.encode().unwrap(),
            b"mg foo v f c t s l h k u T60 N30 R10 C7 E8 Otok\r\n".to_vec()
        );
    }

    #[test]
    fn build_get_validation() {
        let options = GetOptions {
            recache_ttl: Some(0),
            ..GetOptions::default()
        };
        assert!(build_get("foo", &options).is_err());
    }

    #[test]
    fn build_set_modes() {
        let command = build_set("foo", "bar", &SetOptions::default()).unwrap();
        assert_eq!(command.encode().unwrap(), b"ms foo 3\r\nbar\r\n".to_vec());

        let options = SetOptions {
            mode: SetMode::Add,
            ttl: Some(60),
            client_flags: Some(1),
            return_cas: true,
            ..SetOptions::default()
        };
        let command = build_set("foo", "bar", &options).unwrap();
        assert_eq!(command.encode().unwrap(), b"ms foo 3 ME c F1 T60\r\nbar\r\n".to_vec());
    }

    #[test]
    fn build_set_validation() {
        let append_with_ttl = SetOptions {
            mode: SetMode::Append,
            ttl: Some(60),
            ..SetOptions::default()
        };
        assert!(build_set("foo", "bar", &append_with_ttl).is_err());

        let set_with_vivify = SetOptions {
            vivify_ttl: Some(60),
            ..SetOptions::default()
        };
        assert!(build_set("foo", "bar", &set_with_vivify).is_err());

        let add_with_cas = SetOptions {
            mode: SetMode::Add,
            compare_cas: Some(1),
            ..SetOptions::default()
        };
        assert!(build_set("foo", "bar", &add_with_cas).is_err());
    }

    #[test]
    fn build_delete_flags() {
        let command = build_delete("foo", &DeleteOptions::default()).unwrap();
        assert_eq!(command.encode().unwrap(), b"md foo\r\n".to_vec());

        let options = DeleteOptions {
            invalidate: true,
            ttl: Some(30),
            compare_cas: Some(5),
            ..DeleteOptions::default()
        };
        let command = build_delete("foo", &options).unwrap();
        assert_eq!(command.encode().unwrap(), b"md foo I C5 T30\r\n".to_vec());

        let ttl_without_invalidate = DeleteOptions {
            ttl: Some(30),
            ..DeleteOptions::default()
        };
        assert!(build_delete("foo", &ttl_without_invalidate).is_err());
    }

    #[test]
    fn build_arithmetic_flags() {
        let command = build_arithmetic("counter", &ArithmeticOptions::default()).unwrap();
        assert_eq!(command.encode().unwrap(), b"ma counter v\r\n".to_vec());

        let options = ArithmeticOptions {
            mode: ArithmeticMode::Decrement,
            delta: Some(2),
            initial: Some(0),
            initial_ttl: Some(60),
            ..ArithmeticOptions::default()
        };
        let command = build_arithmetic("counter", &options).unwrap();
        assert_eq!(command.encode().unwrap(), b"ma counter MD v D2 N60 J0\r\n".to_vec());

        let initial_without_ttl = ArithmeticOptions {
            initial: Some(0),
            ..ArithmeticOptions::default()
        };
        assert!(build_arithmetic("counter", &initial_without_ttl).is_err());
    }

    #[test]
    fn build_noop_and_debug() {
        assert_eq!(build_noop().encode().unwrap(), b"mn\r\n".to_vec());
        assert_eq!(build_debug("foo").unwrap().encode().unwrap(), b"me foo\r\n".to_vec());
        assert!(build_debug(b"a key".to_vec()).is_err());
    }

    #[test]
    fn opaque_validation() {
        for opaque in [&b""[..], &[b'x'; 33][..], b"a b", b"a\x7f"] {
            let options = GetOptions {
                opaque: Some(opaque.to_vec()),
                ..GetOptions::default()
            };
            assert!(build_get("foo", &options).is_err(), "opaque {:?} should fail", opaque);
        }
    }

    #[test]
    fn parse_meta_result_flags() {
        let mut response = MetaResponse::parse_header(b"VA 3 f1 c42 t-1 h1 l5 s3 W Otok kZm9v b").unwrap();
        response.value = Some(b"bar".to_vec());
        let result = parse_meta_result(response).unwrap();
        assert!(result.ok());
        assert_eq!(result.rc, ReturnCode::Va);
        assert_eq!(result.value, Some(b"bar".to_vec()));
        assert_eq!(result.client_flags, Some(1));
        assert_eq!(result.cas, Some(42));
        assert_eq!(result.ttl, Some(-1));
        assert_eq!(result.hit_before, Some(true));
        assert_eq!(result.last_access, Some(5));
        assert_eq!(result.size, Some(3));
        assert!(result.won);
        assert!(!result.busy);
        assert!(!result.stale);
        assert_eq!(result.opaque, Some(b"tok".to_vec()));
        assert_eq!(result.key, Some(b"foo".to_vec()));
    }

    #[test]
    fn parse_meta_result_miss() {
        let response = MetaResponse::parse_header(b"EN").unwrap();
        let result = parse_meta_result(response).unwrap();
        assert!(!result.ok());
        assert_eq!(result.rc, ReturnCode::En);
    }

    #[test]
    fn parse_debug_result_fields() {
        let response = MetaResponse::parse_header(b"ME foo exp=-1 la=2 cas=3").unwrap();
        let fields = parse_debug_result(&response).unwrap().unwrap();
        assert_eq!(fields.get("exp").map(String::as_str), Some("-1"));
        assert_eq!(fields.get("la").map(String::as_str), Some("2"));
        assert_eq!(fields.get("cas").map(String::as_str), Some("3"));

        let miss = MetaResponse::parse_header(b"EN").unwrap();
        assert!(parse_debug_result(&miss).unwrap().is_none());

        let unexpected = MetaResponse::parse_header(b"HD").unwrap();
        assert!(parse_debug_result(&unexpected).is_err());
    }
}
