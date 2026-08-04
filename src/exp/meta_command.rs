//! Wire-level framing for the memcached meta protocol.
//!
//! [`MetaCommand`] assembles a request line (and optional data block) from a
//! command code, key, flag tokens and value. [`MetaResponse`] parses a
//! response header line. Neither type interprets flags; that belongs to the
//! typed API layer (the `build_*` / `parse_*` functions).

use std::borrow::Cow;

use crate::error::{ClientError, CommandError, MemcacheError, ServerError};

/// memcached limits the key token *on the wire* to 250 bytes and the legacy
/// text key may not contain whitespace or control characters. Keys that
/// violate the latter are sent base64-encoded with the meta `b` flag; since
/// the server checks the length before decoding, the base64 form (not the raw
/// key) is what must fit in 250 bytes, which caps a binary key at ~186 raw
/// bytes.
pub const MAX_KEY_LENGTH: usize = 250;

const BASE64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let triple =
            (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        output.push(BASE64_ALPHABET[(triple >> 18) as usize & 63]);
        output.push(BASE64_ALPHABET[(triple >> 12) as usize & 63]);
        output.push(if chunk.len() > 1 {
            BASE64_ALPHABET[(triple >> 6) as usize & 63]
        } else {
            b'='
        });
        output.push(if chunk.len() > 2 {
            BASE64_ALPHABET[triple as usize & 63]
        } else {
            b'='
        });
    }
    output
}

pub(crate) fn base64_decode(input: &[u8]) -> Result<Vec<u8>, MemcacheError> {
    fn bad_base64() -> MemcacheError {
        ServerError::BadResponse(Cow::Borrowed("invalid base64 token")).into()
    }
    fn value(byte: u8) -> Result<u32, MemcacheError> {
        match byte {
            b'A'..=b'Z' => Ok((byte - b'A') as u32),
            b'a'..=b'z' => Ok((byte - b'a') as u32 + 26),
            b'0'..=b'9' => Ok((byte - b'0') as u32 + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(bad_base64()),
        }
    }
    if !input.len().is_multiple_of(4) {
        return Err(bad_base64());
    }
    let mut output = Vec::with_capacity(input.len() / 4 * 3);
    for chunk in input.chunks(4) {
        let padding = chunk.iter().rev().take_while(|&&byte| byte == b'=').count();
        if padding > 2 || (padding > 0 && !std::ptr::eq(chunk, input.chunks(4).last().unwrap())) {
            return Err(bad_base64());
        }
        let mut triple = 0u32;
        for (index, &byte) in chunk.iter().enumerate() {
            let bits = if byte == b'=' {
                // Only the trailing padding positions may hold '='.
                if index < chunk.len() - padding {
                    return Err(bad_base64());
                }
                0
            } else {
                value(byte)?
            };
            triple |= bits << (18 - index * 6);
        }
        output.push((triple >> 16) as u8);
        if padding < 2 {
            output.push((triple >> 8) as u8);
        }
        if padding < 1 {
            output.push(triple as u8);
        }
    }
    Ok(output)
}

/// Whether `key` can be sent verbatim (no whitespace/control chars).
fn is_legacy_safe(key: &[u8]) -> bool {
    key.iter().all(|&byte| byte > 0x20 && byte != 0x7f)
}

/// Return `(wire_key, needs_base64_flag)` for a raw key.
pub(crate) fn encode_key(key: &[u8]) -> Result<(Cow<'_, [u8]>, bool), MemcacheError> {
    let (wire_key, needs_base64) = if is_legacy_safe(key) {
        (Cow::Borrowed(key), false)
    } else {
        (Cow::Owned(base64_encode(key)), true)
    };
    if wire_key.len() > MAX_KEY_LENGTH {
        return Err(ClientError::KeyTooLong.into());
    }
    Ok((wire_key, needs_base64))
}

/// A meta protocol command code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetaOp {
    /// `mg` - meta get.
    Get,
    /// `ms` - meta set.
    Set,
    /// `md` - meta delete.
    Delete,
    /// `ma` - meta arithmetic.
    Arithmetic,
    /// `mn` - meta no-op, used as a pipeline barrier.
    Noop,
    /// `me` - meta debug.
    Debug,
}

impl MetaOp {
    pub fn wire(self) -> &'static [u8] {
        match self {
            MetaOp::Get => b"mg",
            MetaOp::Set => b"ms",
            MetaOp::Delete => b"md",
            MetaOp::Arithmetic => b"ma",
            MetaOp::Noop => b"mn",
            MetaOp::Debug => b"me",
        }
    }
}

/// A meta protocol response return code.
///
/// Non-exhaustive: memcached adds return codes over time, so matches need a
/// wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReturnCode {
    /// `HD` - success without a value block.
    Hd,
    /// `VA` - success with a value block.
    Va,
    /// `EN` - miss.
    En,
    /// `NS` - not stored.
    Ns,
    /// `EX` - item exists (CAS mismatch).
    Ex,
    /// `NF` - not found.
    Nf,
    /// `MN` - no-op marker.
    Mn,
    /// `ME` - debug line.
    Me,
}

impl ReturnCode {
    pub fn from_wire(token: &[u8]) -> Result<ReturnCode, MemcacheError> {
        match token {
            b"HD" => Ok(ReturnCode::Hd),
            b"VA" => Ok(ReturnCode::Va),
            b"EN" => Ok(ReturnCode::En),
            b"NS" => Ok(ReturnCode::Ns),
            b"EX" => Ok(ReturnCode::Ex),
            b"NF" => Ok(ReturnCode::Nf),
            b"MN" => Ok(ReturnCode::Mn),
            b"ME" => Ok(ReturnCode::Me),
            _ => Err(ServerError::BadResponse(Cow::Owned(format!(
                "unknown return code {:?}",
                String::from_utf8_lossy(token)
            )))
            .into()),
        }
    }
}

/// A meta protocol request: command code, key, flag tokens and optional data
/// block. The data length token is derived from the value; the base64 `b`
/// flag is added automatically when the key requires it.
///
/// Construct with [`new`](Self::new) and chain [`flag`](Self::flag) /
/// [`value`](Self::value); the `build_*` functions in
/// this module's typed API cover the common commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaCommand {
    pub(crate) op: MetaOp,
    pub(crate) key: Vec<u8>,
    pub(crate) flags: Vec<Vec<u8>>,
    pub(crate) value: Option<Vec<u8>>,
}

impl MetaCommand {
    pub fn new(op: MetaOp, key: impl Into<Vec<u8>>) -> MetaCommand {
        MetaCommand {
            op,
            key: key.into(),
            flags: Vec::new(),
            value: None,
        }
    }

    /// Append a raw flag token (e.g. `"v"` or `"T60"`).
    #[must_use]
    pub fn flag(mut self, token: impl Into<Vec<u8>>) -> MetaCommand {
        self.flags.push(token.into());
        self
    }

    /// Set the data block sent after the request line.
    #[must_use]
    pub fn value(mut self, value: impl Into<Vec<u8>>) -> MetaCommand {
        self.value = Some(value.into());
        self
    }

    /// Encode the full request (header line plus data block) to wire bytes.
    pub fn encode(&self) -> Result<Vec<u8>, MemcacheError> {
        let mut buffer = Vec::new();
        self.encode_into(&mut buffer)?;
        Ok(buffer)
    }

    pub fn encode_into(&self, buffer: &mut Vec<u8>) -> Result<(), MemcacheError> {
        buffer.extend_from_slice(self.op.wire());
        if self.op == MetaOp::Noop {
            // mn takes no key, flags or value.
            buffer.extend_from_slice(b"\r\n");
            return Ok(());
        }
        if self.key.is_empty() {
            return Err(ClientError::Error(Cow::Borrowed("key must not be empty")).into());
        }
        let (wire_key, needs_base64) = encode_key(&self.key)?;
        buffer.push(b' ');
        buffer.extend_from_slice(&wire_key);
        if let Some(value) = &self.value {
            buffer.push(b' ');
            buffer.extend_from_slice(value.len().to_string().as_bytes());
        }
        for flag in &self.flags {
            buffer.push(b' ');
            buffer.extend_from_slice(flag);
        }
        if needs_base64 && !self.flags.iter().any(|flag| flag.as_slice() == b"b") {
            buffer.extend_from_slice(b" b");
        }
        buffer.extend_from_slice(b"\r\n");
        if let Some(value) = &self.value {
            buffer.extend_from_slice(value);
            buffer.extend_from_slice(b"\r\n");
        }
        Ok(())
    }
}

/// A lightly framed meta protocol response: return code, raw flag tokens and
/// (for `VA`) the data block, filled in by the transport after it reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaResponse {
    pub(crate) rc: ReturnCode,
    pub(crate) datalen: Option<usize>,
    pub(crate) flags: Vec<Vec<u8>>,
    pub(crate) value: Option<Vec<u8>>,
}

impl MetaResponse {
    /// The wire return code.
    pub fn rc(&self) -> ReturnCode {
        self.rc
    }

    /// The raw response flag tokens.
    pub fn flags(&self) -> impl Iterator<Item = &[u8]> {
        self.flags.iter().map(Vec::as_slice)
    }

    /// The data block of a `VA` response.
    pub fn value(&self) -> Option<&[u8]> {
        self.value.as_deref()
    }

    /// Consume the response and take the data block of a `VA` response.
    pub fn into_value(self) -> Option<Vec<u8>> {
        self.value
    }

    /// Parse a response header line (without the trailing CRLF).
    ///
    /// Legacy `ERROR` / `CLIENT_ERROR` / `SERVER_ERROR` lines are converted
    /// to the corresponding [`MemcacheError`].
    pub fn parse_header(line: &[u8]) -> Result<MetaResponse, MemcacheError> {
        let mut tokens = line.split(|&byte| byte == b' ').filter(|token| !token.is_empty());
        let rc_token = tokens
            .next()
            .ok_or(ServerError::BadResponse(Cow::Borrowed("empty response line")))?;
        match rc_token {
            b"ERROR" => return Err(CommandError::InvalidCommand.into()),
            b"CLIENT_ERROR" | b"SERVER_ERROR" => {
                let message = String::from_utf8_lossy(&line[rc_token.len()..]).trim().to_string();
                if rc_token == b"CLIENT_ERROR" {
                    return Err(ClientError::Error(Cow::Owned(message)).into());
                }
                return Err(ServerError::Error(message).into());
            }
            _ => {}
        }
        let rc = ReturnCode::from_wire(rc_token)?;
        let mut flags: Vec<Vec<u8>> = tokens.map(|token| token.to_vec()).collect();
        let mut datalen = None;
        // Only VA carries a datalen token; ME responses echo the key, which
        // may itself start with a digit.
        if rc == ReturnCode::Va {
            if flags.is_empty() {
                return Err(ServerError::BadResponse(Cow::Borrowed("VA response missing data length")).into());
            }
            let token = flags.remove(0);
            datalen = Some(std::str::from_utf8(&token)?.parse::<usize>()?);
        }
        Ok(MetaResponse {
            rc,
            datalen,
            flags,
            value: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        let cases: &[&[u8]] = &[b"", b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", b"\x00\xff\x10"];
        for case in cases {
            let encoded = base64_encode(case);
            assert_eq!(base64_decode(&encoded).unwrap(), case.to_vec());
        }
        assert_eq!(base64_encode(b"foobar"), b"Zm9vYmFy".to_vec());
        assert_eq!(base64_encode(b"foob"), b"Zm9vYg==".to_vec());
        assert!(base64_decode(b"Zm9vY").is_err());
        assert!(base64_decode(b"Zm=vYg==").is_err());
    }

    #[test]
    fn encode_get() {
        let command = MetaCommand::new(MetaOp::Get, "foo").flag("v").flag("t");
        assert_eq!(command.encode().unwrap(), b"mg foo v t\r\n".to_vec());
    }

    #[test]
    fn encode_set_with_value() {
        let command = MetaCommand::new(MetaOp::Set, "foo").flag("T60").value("bar");
        assert_eq!(command.encode().unwrap(), b"ms foo 3 T60\r\nbar\r\n".to_vec());
    }

    #[test]
    fn encode_noop() {
        let command = MetaCommand::new(MetaOp::Noop, "");
        assert_eq!(command.encode().unwrap(), b"mn\r\n".to_vec());
    }

    #[test]
    fn encode_binary_key_adds_base64_flag() {
        let command = MetaCommand::new(MetaOp::Get, b"a key".to_vec()).flag("v");
        assert_eq!(command.encode().unwrap(), b"mg YSBrZXk= v b\r\n".to_vec());
    }

    #[test]
    fn encode_rejects_bad_keys() {
        assert!(MetaCommand::new(MetaOp::Get, "").encode().is_err());
        let long_key = vec![b'x'; MAX_KEY_LENGTH + 1];
        assert!(MetaCommand::new(MetaOp::Get, long_key).encode().is_err());
        // The base64 form is what must fit in 250 bytes.
        let binary_key = vec![b' '; 200];
        assert!(MetaCommand::new(MetaOp::Get, binary_key).encode().is_err());
    }

    #[test]
    fn parse_header_hd_with_flags() {
        let response = MetaResponse::parse_header(b"HD c1234 t42").unwrap();
        assert_eq!(response.rc, ReturnCode::Hd);
        assert_eq!(response.datalen, None);
        assert_eq!(response.flags, vec![b"c1234".to_vec(), b"t42".to_vec()]);
    }

    #[test]
    fn parse_header_va_datalen() {
        let response = MetaResponse::parse_header(b"VA 5 f0 c1").unwrap();
        assert_eq!(response.rc, ReturnCode::Va);
        assert_eq!(response.datalen, Some(5));
        assert_eq!(response.flags, vec![b"f0".to_vec(), b"c1".to_vec()]);
        assert!(MetaResponse::parse_header(b"VA").is_err());
    }

    #[test]
    fn parse_header_errors() {
        assert!(matches!(
            MetaResponse::parse_header(b"ERROR"),
            Err(MemcacheError::CommandError(CommandError::InvalidCommand))
        ));
        assert!(matches!(
            MetaResponse::parse_header(b"CLIENT_ERROR bad data chunk"),
            Err(MemcacheError::ClientError(_))
        ));
        assert!(matches!(
            MetaResponse::parse_header(b"SERVER_ERROR out of memory"),
            Err(MemcacheError::ServerError(_))
        ));
        assert!(MetaResponse::parse_header(b"XX").is_err());
        assert!(MetaResponse::parse_header(b"").is_err());
    }
}
