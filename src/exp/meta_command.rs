//! Wire-level framing for the memcached meta protocol.
//!
//! [`MetaCommand`] assembles a request line (and optional data block) from a
//! command code, key, flag tokens and value. [`MetaResponse`] parses a
//! response header line. Neither type interprets flags; that belongs to the
//! typed API layer (the `build_*` / `parse_*` functions).

use std::borrow::Cow;

use super::error::{Error, Result};

/// memcached limits the key token *on the wire* to 250 bytes and the legacy
/// text key may not contain whitespace or control characters. Keys that
/// violate the latter are sent base64-encoded with the meta `b` flag; since
/// the server checks the length before decoding, the base64 form (not the raw
/// key) is what must fit in 250 bytes, which caps a binary key at ~186 raw
/// bytes.
pub const MAX_KEY_LENGTH: usize = 250;

/// The largest data block a `VA` response may announce: memcached's own
/// item size ceiling (`-I` caps at 1 GiB). A larger length is a protocol
/// violation or a desynchronized stream, and is rejected instead of being
/// allocated.
pub const MAX_VALUE_LENGTH: usize = 1 << 30;

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

pub(crate) fn base64_decode(input: &[u8]) -> Result<Vec<u8>> {
    fn bad_base64() -> Error {
        Error::protocol("invalid base64 token")
    }
    fn value(byte: u8) -> Result<u32> {
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
pub(crate) fn encode_key(key: &[u8]) -> Result<(Cow<'_, [u8]>, bool)> {
    let (wire_key, needs_base64) = if is_legacy_safe(key) {
        (Cow::Borrowed(key), false)
    } else {
        (Cow::Owned(base64_encode(key)), true)
    };
    if wire_key.len() > MAX_KEY_LENGTH {
        return Err(Error::Usage("key exceeds 250 bytes on the wire"));
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
    pub fn from_wire(token: &[u8]) -> Result<ReturnCode> {
        match token {
            b"HD" => Ok(ReturnCode::Hd),
            b"VA" => Ok(ReturnCode::Va),
            b"EN" => Ok(ReturnCode::En),
            b"NS" => Ok(ReturnCode::Ns),
            b"EX" => Ok(ReturnCode::Ex),
            b"NF" => Ok(ReturnCode::Nf),
            b"MN" => Ok(ReturnCode::Mn),
            b"ME" => Ok(ReturnCode::Me),
            _ => Err(Error::Protocol(format!(
                "unknown return code {:?}",
                String::from_utf8_lossy(token)
            ))),
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

    /// Whether the command can change server state: every write, and a
    /// read that touches (`T`), vivifies (`N`), takes a recache lease (`R`)
    /// or rewrites the CAS (`E`). Decides whether a request written but
    /// unanswered is [`Error::Ambiguous`].
    pub fn has_side_effect(&self) -> bool {
        match self.op {
            MetaOp::Set | MetaOp::Delete | MetaOp::Arithmetic => true,
            MetaOp::Noop | MetaOp::Debug => false,
            MetaOp::Get => self
                .flags
                .iter()
                .any(|flag| matches!(flag.first(), Some(b'T' | b'N' | b'R' | b'E'))),
        }
    }

    /// Check the key without encoding, so a usage error surfaces before a
    /// connection is dialed.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.op == MetaOp::Noop {
            return Ok(());
        }
        if self.key.is_empty() {
            return Err(Error::Usage("key must not be empty"));
        }
        encode_key(&self.key).map(|_| ())
    }

    /// Encode the full request (header line plus data block) to wire bytes.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut buffer = Vec::new();
        self.encode_into(&mut buffer)?;
        Ok(buffer)
    }

    pub fn encode_into(&self, buffer: &mut Vec<u8>) -> Result<()> {
        self.encode_flagged(None, buffer)
    }

    /// Encode with an `O<position>` opaque token appended to the flags,
    /// without copying the value into a flagged clone first.
    pub(crate) fn encode_with_opaque(&self, position: usize, buffer: &mut Vec<u8>) -> Result<()> {
        self.encode_flagged(Some(position), buffer)
    }

    fn encode_flagged(&self, opaque: Option<usize>, buffer: &mut Vec<u8>) -> Result<()> {
        buffer.extend_from_slice(self.op.wire());
        if self.op == MetaOp::Noop {
            // mn takes no key, flags or value.
            buffer.extend_from_slice(b"\r\n");
            return Ok(());
        }
        self.validate()?;
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
        if let Some(position) = opaque {
            buffer.extend_from_slice(format!(" O{position}").as_bytes());
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
    /// Legacy `ERROR` / `CLIENT_ERROR` lines become [`Error::Rejected`]
    /// and `SERVER_ERROR` lines [`Error::Server`].
    pub fn parse_header(line: &[u8]) -> Result<MetaResponse> {
        let mut tokens = line.split(|&byte| byte == b' ').filter(|token| !token.is_empty());
        let rc_token = tokens.next().ok_or_else(|| Error::protocol("empty response line"))?;
        match rc_token {
            b"ERROR" => return Err(Error::Rejected("ERROR".to_string())),
            b"CLIENT_ERROR" | b"SERVER_ERROR" => {
                let message = String::from_utf8_lossy(&line[rc_token.len()..]).trim().to_string();
                if rc_token == b"CLIENT_ERROR" {
                    return Err(Error::Rejected(message));
                }
                return Err(Error::Server(message));
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
                return Err(Error::protocol("VA response missing data length"));
            }
            let token = flags.remove(0);
            let length = std::str::from_utf8(&token)
                .ok()
                .and_then(|token| token.parse::<usize>().ok())
                .ok_or_else(|| Error::protocol("VA response has an invalid data length"))?;
            if length > MAX_VALUE_LENGTH {
                return Err(Error::Protocol(format!(
                    "VA response announces {length} bytes, above the {MAX_VALUE_LENGTH} byte limit"
                )));
            }
            datalen = Some(length);
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
    fn va_length_is_bounded() {
        assert_eq!(MetaResponse::parse_header(b"VA 3 c7").unwrap().datalen, Some(3));
        assert_eq!(
            MetaResponse::parse_header(b"VA 1073741824").unwrap().datalen,
            Some(MAX_VALUE_LENGTH)
        );
        for header in [
            b"VA 1073741825".as_slice(),
            b"VA 18446744073709551615",
            b"VA 99999999999999999999",
        ] {
            let error = MetaResponse::parse_header(header).unwrap_err();
            assert!(matches!(error, Error::Protocol(_)), "{header:?}: {error:?}");
        }
    }

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
    fn encode_binary_key_adds_base64_flag() {
        let command = MetaCommand::new(MetaOp::Get, b"a key".to_vec()).flag("v");
        assert_eq!(command.encode().unwrap(), b"mg YSBrZXk= v b\r\n".to_vec());
    }

    #[test]
    fn encode_rejects_bad_keys() {
        assert!(matches!(
            MetaCommand::new(MetaOp::Get, "").encode(),
            Err(Error::Usage(_))
        ));
        let long_key = vec![b'x'; MAX_KEY_LENGTH + 1];
        assert!(matches!(
            MetaCommand::new(MetaOp::Get, long_key).encode(),
            Err(Error::Usage(_))
        ));
        // The base64 form is what must fit in 250 bytes.
        let binary_key = vec![b' '; 200];
        assert!(matches!(
            MetaCommand::new(MetaOp::Get, binary_key).encode(),
            Err(Error::Usage(_))
        ));
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
        assert!(matches!(MetaResponse::parse_header(b"ERROR"), Err(Error::Rejected(_))));
        assert!(matches!(
            MetaResponse::parse_header(b"CLIENT_ERROR bad data chunk"),
            Err(Error::Rejected(message)) if message == "bad data chunk"
        ));
        assert!(matches!(
            MetaResponse::parse_header(b"SERVER_ERROR out of memory"),
            Err(Error::Server(message)) if message == "out of memory"
        ));
        assert!(matches!(MetaResponse::parse_header(b"XX"), Err(Error::Protocol(_))));
        assert!(matches!(MetaResponse::parse_header(b""), Err(Error::Protocol(_))));
        assert!(matches!(MetaResponse::parse_header(b"VA x"), Err(Error::Protocol(_))));
    }
}
