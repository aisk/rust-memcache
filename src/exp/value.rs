//! The object value model: [`Encode`] turns a value into the bytes and
//! client flags stored with an item, [`Decode`] turns them back into the
//! type the caller asks for.
//!
//! Both traits live on the value type, not on the client, so each call
//! site picks its own encoding by type inference (`get::<User>(key)`) and
//! the client stays a single, non-generic type. The built-in
//! implementations store bytes verbatim ([`FLAG_BYTES`], zero), strings
//! verbatim tagged [`FLAG_STR`], and integers as decimal ASCII tagged
//! [`FLAG_INT`], which keeps them usable by `incr` / `decr`. The flag
//! values follow the sibling Python client so the same item can be read
//! by both.
//!
//! Decoding is type-driven. The built-in decoders ignore the stored flags
//! and decode whatever bytes parse, so an item written by another client
//! under a different flag convention still reads; flags are advisory
//! metadata for custom implementations (a compression marker, a format
//! tag). This is a contract: a future release will not make the built-in
//! decoders reject flags they do not recognize.
//!
//! References encode through a blanket impl, so `set(key, &user, ttl)`
//! borrows and `set(key, user, ttl)` moves; both work. With the
//! `serde_json` feature, [`Json<T>`] wraps any `serde` type.

use std::error::Error as StdError;
use std::fmt;

/// Untyped raw bytes (the protocol default of zero).
pub const FLAG_BYTES: u32 = 0;
/// An integer stored as decimal ASCII.
pub const FLAG_INT: u32 = 1 << 1;
/// A UTF-8 string.
pub const FLAG_STR: u32 = 1 << 4;
/// A JSON document (the [`Json`] wrapper, `serde_json` feature).
pub const FLAG_JSON: u32 = 1 << 5;

type BoxError = Box<dyn StdError + Send + Sync>;

/// An encoded value: the bytes stored and the client flags stored with
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub flags: u32,
}

impl Encoded {
    pub fn new(bytes: impl Into<Vec<u8>>, flags: u32) -> Encoded {
        Encoded {
            bytes: bytes.into(),
            flags,
        }
    }
}

/// Encode a value for storage.
pub trait Encode {
    fn encode(&self) -> Result<Encoded, EncodeError>;
}

/// Decode a stored value, driven by the requested type.
pub trait Decode: Sized {
    fn decode(bytes: Vec<u8>, flags: u32) -> Result<Self, DecodeError>;
}

/// An [`Encode`] implementation failed.
#[derive(Debug)]
pub struct EncodeError {
    source: BoxError,
}

impl EncodeError {
    pub fn new(source: impl Into<BoxError>) -> EncodeError {
        EncodeError { source: source.into() }
    }

    /// A copy that keeps the rendered message but not the source type.
    pub(crate) fn duplicate(&self) -> EncodeError {
        EncodeError::new(self.source.to_string())
    }
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "encode failed: {}", self.source)
    }
}

impl StdError for EncodeError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.source.as_ref())
    }
}

/// A [`Decode`] implementation failed.
#[derive(Debug)]
pub struct DecodeError {
    source: BoxError,
}

impl DecodeError {
    pub fn new(source: impl Into<BoxError>) -> DecodeError {
        DecodeError { source: source.into() }
    }

    /// A copy that keeps the rendered message but not the source type.
    pub(crate) fn duplicate(&self) -> DecodeError {
        DecodeError::new(self.source.to_string())
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "decode failed: {}", self.source)
    }
}

impl StdError for DecodeError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.source.as_ref())
    }
}

impl<T: Encode + ?Sized> Encode for &T {
    fn encode(&self) -> Result<Encoded, EncodeError> {
        (**self).encode()
    }
}

impl Encode for [u8] {
    fn encode(&self) -> Result<Encoded, EncodeError> {
        Ok(Encoded::new(self, FLAG_BYTES))
    }
}

impl Encode for Vec<u8> {
    fn encode(&self) -> Result<Encoded, EncodeError> {
        Ok(Encoded::new(self.as_slice(), FLAG_BYTES))
    }
}

impl<const N: usize> Encode for [u8; N] {
    fn encode(&self) -> Result<Encoded, EncodeError> {
        Ok(Encoded::new(self.as_slice(), FLAG_BYTES))
    }
}

impl Encode for str {
    fn encode(&self) -> Result<Encoded, EncodeError> {
        Ok(Encoded::new(self.as_bytes(), FLAG_STR))
    }
}

impl Encode for String {
    fn encode(&self) -> Result<Encoded, EncodeError> {
        self.as_str().encode()
    }
}

impl Decode for Vec<u8> {
    fn decode(bytes: Vec<u8>, _flags: u32) -> Result<Self, DecodeError> {
        Ok(bytes)
    }
}

impl Decode for String {
    fn decode(bytes: Vec<u8>, _flags: u32) -> Result<Self, DecodeError> {
        String::from_utf8(bytes).map_err(DecodeError::new)
    }
}

macro_rules! impl_integer {
    ($($ty:ident)*) => {$(
        impl Encode for $ty {
            fn encode(&self) -> Result<Encoded, EncodeError> {
                Ok(Encoded::new(self.to_string(), FLAG_INT))
            }
        }

        impl Decode for $ty {
            fn decode(bytes: Vec<u8>, _flags: u32) -> Result<Self, DecodeError> {
                std::str::from_utf8(&bytes)
                    .map_err(DecodeError::new)?
                    .parse::<$ty>()
                    .map_err(DecodeError::new)
            }
        }
    )*};
}

impl_integer!(u8 u16 u32 u64 u128 usize i8 i16 i32 i64 i128 isize);

/// Store a `serde` type as JSON, tagged [`FLAG_JSON`] (`serde_json`
/// feature). `Json(&value)` borrows for a write; `get::<Json<T>>` decodes
/// an owned `T`, reachable through [`into_inner`](Self::into_inner) or
/// `Deref`.
///
/// ```no_run
/// # #[cfg(feature = "serde_json")] {
/// use memcache::exp::Json;
/// #[derive(serde::Serialize, serde::Deserialize)]
/// struct User { name: String }
/// let user = User { name: "ann".into() };
/// let encoded = memcache::exp::Encode::encode(&Json(&user)).unwrap();
/// assert_eq!(encoded.flags, memcache::exp::FLAG_JSON);
/// # }
/// ```
#[cfg(feature = "serde_json")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Json<T>(pub T);

#[cfg(feature = "serde_json")]
impl<T> Json<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

#[cfg(feature = "serde_json")]
impl<T> From<T> for Json<T> {
    fn from(value: T) -> Json<T> {
        Json(value)
    }
}

#[cfg(feature = "serde_json")]
impl<T> std::ops::Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

#[cfg(feature = "serde_json")]
impl<T> std::ops::DerefMut for Json<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

#[cfg(feature = "serde_json")]
impl<T: serde::Serialize> Encode for Json<T> {
    fn encode(&self) -> Result<Encoded, EncodeError> {
        let bytes = serde_json::to_vec(&self.0).map_err(EncodeError::new)?;
        Ok(Encoded::new(bytes, FLAG_JSON))
    }
}

#[cfg(feature = "serde_json")]
impl<T: serde::de::DeserializeOwned> Decode for Json<T> {
    fn decode(bytes: Vec<u8>, _flags: u32) -> Result<Self, DecodeError> {
        serde_json::from_slice(&bytes).map(Json).map_err(DecodeError::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded<V: Encode>(value: V) -> (Vec<u8>, u32) {
        let Encoded { bytes, flags } = value.encode().unwrap();
        (bytes, flags)
    }

    #[test]
    // References are encoded through the blanket impl; that is the point.
    #[allow(clippy::needless_borrows_for_generic_args)]
    fn encode_builtin_types() {
        assert_eq!(encoded("bar"), (b"bar".to_vec(), FLAG_STR));
        assert_eq!(encoded(String::from("bar")), (b"bar".to_vec(), FLAG_STR));
        assert_eq!(encoded(&String::from("bar")), (b"bar".to_vec(), FLAG_STR));
        assert_eq!(encoded(b"bar"), (b"bar".to_vec(), FLAG_BYTES));
        assert_eq!(encoded(*b"bar"), (b"bar".to_vec(), FLAG_BYTES));
        assert_eq!(encoded(&b"bar"[..]), (b"bar".to_vec(), FLAG_BYTES));
        assert_eq!(encoded(vec![1u8, 2]), (vec![1, 2], FLAG_BYTES));
        assert_eq!(encoded(&vec![1u8, 2]), (vec![1, 2], FLAG_BYTES));
        assert_eq!(encoded(42u64), (b"42".to_vec(), FLAG_INT));
        assert_eq!(encoded(&-1i32), (b"-1".to_vec(), FLAG_INT));
        // Empty encodings are produced here; the high-level layer rejects them.
        assert_eq!(encoded(""), (Vec::new(), FLAG_STR));
    }

    #[test]
    fn decode_is_type_driven() {
        assert_eq!(String::decode(b"bar".to_vec(), FLAG_STR).unwrap(), "bar");
        assert_eq!(u64::decode(b"42".to_vec(), FLAG_INT).unwrap(), 42);
        assert_eq!(i8::decode(b"-5".to_vec(), FLAG_INT).unwrap(), -5);
        assert!(u64::decode(b"x".to_vec(), FLAG_INT).is_err());
        assert!(String::decode(vec![0xff], FLAG_STR).is_err());

        // Flags are advisory: any value decodes as long as the bytes parse.
        assert_eq!(String::decode(b"42".to_vec(), FLAG_INT).unwrap(), "42");
        assert_eq!(String::decode(b"\"x\"".to_vec(), FLAG_JSON).unwrap(), "\"x\"");
        assert_eq!(u64::decode(b"42".to_vec(), FLAG_STR).unwrap(), 42);
        assert_eq!(Vec::<u8>::decode(b"x".to_vec(), 1).unwrap(), b"x".to_vec());
    }

    #[test]
    fn errors_render_their_source() {
        let error = u8::decode(b"300".to_vec(), FLAG_INT).unwrap_err();
        assert!(error.to_string().starts_with("decode failed: "));
        assert!(StdError::source(&error).is_some());
        let error = EncodeError::new("custom");
        assert_eq!(error.to_string(), "encode failed: custom");
    }

    #[cfg(feature = "serde_json")]
    #[test]
    fn json_roundtrip() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        struct User {
            name: String,
        }
        let user = User { name: "ann".into() };
        let (bytes, flags) = encoded(Json(&user));
        assert_eq!(flags, FLAG_JSON);
        assert_eq!(bytes, br#"{"name":"ann"}"#);
        let decoded: Json<User> = Json::decode(bytes, FLAG_JSON).unwrap();
        assert_eq!(decoded.name, "ann");
        assert_eq!(decoded.into_inner(), user);
        assert!(Json::<User>::decode(b"{".to_vec(), FLAG_JSON).is_err());
    }
}
