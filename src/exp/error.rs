//! The error type of the `exp` module.
//!
//! Semantic outcomes (a miss, a CAS mismatch, a lease state) are never
//! errors; the protocol layer reports them in its result types and the
//! scenario verbs consume them. What remains is split by what the caller
//! can do about it: transport failures where the request was definitely
//! not applied ([`Io`](Error::Io), [`Timeout`](Error::Timeout)) are
//! retryable; a request that was written but whose outcome is unknown is
//! [`Ambiguous`](Error::Ambiguous) and must not be retried blindly; usage
//! errors ([`Usage`](Error::Usage), [`EmptyValue`](Error::EmptyValue),
//! [`Encode`](Error::Encode), [`Decode`](Error::Decode)) are caller bugs;
//! and [`Callback`](Error::Callback) carries the caller's own error out of
//! a loader or update function.

use std::fmt;
use std::io;
use std::sync::Arc;

use super::value::{DecodeError, EncodeError};

/// A specialized `Result` with the module's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What can go wrong in `exp`. See the [module docs](self).
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Transport failure before the request was fully written: it was
    /// definitely not applied.
    Io(io::Error),
    /// The exchange deadline passed before the request was written.
    Timeout { key: Vec<u8> },
    /// The request was written but the outcome could not be observed.
    /// Retrying may duplicate the effect.
    Ambiguous { key: Vec<u8>, source: Box<Error> },
    /// A malformed or unexpected response.
    Protocol(String),
    /// A `SERVER_ERROR` response.
    Server(String),
    /// A `CLIENT_ERROR` (or bare `ERROR`) response: the server rejected
    /// the command as malformed. A usage error seen by the server, never
    /// retried.
    Rejected(String),
    /// `update` or `take` kept losing to concurrent writers and ran out of
    /// retries.
    Conflict,
    /// The value encoded to zero bytes. Zero-byte items are reserved as
    /// lease placeholders, so the scenario layer refuses to store them.
    EmptyValue,
    /// The value could not be encoded.
    Encode(EncodeError),
    /// The stored bytes could not be decoded into the requested type.
    Decode(DecodeError),
    /// An invalid argument. Never retried.
    Usage(&'static str),
    /// The caller's loader or update function failed. Shared across all
    /// waiters of a singleflight, hence the `Arc`; downcast with
    /// [`callback`](Self::callback).
    Callback(Arc<dyn std::error::Error + Send + Sync>),
}

impl Error {
    /// Whether the outcome of a written request is unknown.
    pub fn is_ambiguous(&self) -> bool {
        matches!(self, Error::Ambiguous { .. })
    }

    /// Whether retrying the same request is safe: the request was
    /// definitely not applied. Never true for [`Ambiguous`](Error::Ambiguous).
    pub fn is_retryable(&self) -> bool {
        matches!(self, Error::Io(_) | Error::Timeout { .. })
    }

    /// The caller's own error inside a [`Callback`](Error::Callback), if
    /// it is an `E`.
    pub fn callback<E: std::error::Error + 'static>(&self) -> Option<&E> {
        match self {
            Error::Callback(source) => source.downcast_ref::<E>(),
            _ => None,
        }
    }

    pub(crate) fn protocol(message: impl Into<String>) -> Error {
        Error::Protocol(message.into())
    }

    /// Attribute a keyless transport error to the key it was sent for:
    /// an io timeout becomes [`Timeout`](Error::Timeout).
    pub(crate) fn for_key(self, key: &[u8]) -> Error {
        match self {
            Error::Io(error) if error.kind() == io::ErrorKind::TimedOut => Error::Timeout { key: key.to_vec() },
            other => other,
        }
    }

    /// Best-effort duplicate, so one failure covering a whole batch group
    /// can be reported on each operation of the group. io errors keep
    /// their kind and message but lose the source chain; encode and decode
    /// errors keep their rendered message.
    pub(crate) fn duplicate(&self) -> Error {
        match self {
            Error::Io(error) => Error::Io(io::Error::new(error.kind(), error.to_string())),
            Error::Timeout { key } => Error::Timeout { key: key.clone() },
            Error::Ambiguous { key, source } => Error::Ambiguous {
                key: key.clone(),
                source: Box::new(source.duplicate()),
            },
            Error::Protocol(message) => Error::Protocol(message.clone()),
            Error::Server(message) => Error::Server(message.clone()),
            Error::Rejected(message) => Error::Rejected(message.clone()),
            Error::Conflict => Error::Conflict,
            Error::EmptyValue => Error::EmptyValue,
            Error::Encode(error) => Error::Encode(error.duplicate()),
            Error::Decode(error) => Error::Decode(error.duplicate()),
            Error::Usage(message) => Error::Usage(message),
            Error::Callback(source) => Error::Callback(Arc::clone(source)),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(error) => write!(f, "i/o error: {error}"),
            Error::Timeout { key } => write!(f, "timed out before sending request for {:?}", Key(key)),
            Error::Ambiguous { key, source } => {
                write!(f, "outcome of request for {:?} is unknown: {source}", Key(key))
            }
            Error::Protocol(message) => write!(f, "protocol error: {message}"),
            Error::Server(message) => write!(f, "server error: {message}"),
            Error::Rejected(message) => write!(f, "command rejected by server: {message}"),
            Error::Conflict => write!(f, "too many conflicting concurrent writes"),
            Error::EmptyValue => write!(f, "empty values are reserved as lease placeholders"),
            Error::Encode(error) => error.fmt(f),
            Error::Decode(error) => error.fmt(f),
            Error::Usage(message) => write!(f, "invalid argument: {message}"),
            Error::Callback(error) => write!(f, "callback failed: {error}"),
        }
    }
}

/// Keys are usually text; render them as such when they are.
struct Key<'a>(&'a [u8]);

impl fmt::Debug for Key<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match std::str::from_utf8(self.0) {
            Ok(text) => fmt::Debug::fmt(text, f),
            Err(_) => fmt::Debug::fmt(self.0, f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(error) => Some(error),
            Error::Ambiguous { source, .. } => Some(source.as_ref()),
            Error::Encode(error) => Some(error),
            Error::Decode(error) => Some(error),
            Error::Callback(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(error)
    }
}

impl From<EncodeError> for Error {
    fn from(error: EncodeError) -> Error {
        Error::Encode(error)
    }
}

impl From<DecodeError> for Error {
    fn from(error: DecodeError) -> Error {
        Error::Decode(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct LoadFailed;

    impl fmt::Display for LoadFailed {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("load failed")
        }
    }

    impl std::error::Error for LoadFailed {}

    #[test]
    fn classification() {
        assert!(Error::Io(io::Error::from(io::ErrorKind::ConnectionReset)).is_retryable());
        assert!(Error::Timeout { key: b"k".to_vec() }.is_retryable());
        let ambiguous = Error::Ambiguous {
            key: b"k".to_vec(),
            source: Box::new(Error::Io(io::Error::from(io::ErrorKind::TimedOut))),
        };
        assert!(ambiguous.is_ambiguous());
        assert!(!ambiguous.is_retryable());
        assert!(!Error::Conflict.is_retryable());
        assert!(!Error::Usage("x").is_retryable());
    }

    #[test]
    fn callback_downcast() {
        let error = Error::Callback(Arc::new(LoadFailed));
        assert!(error.callback::<LoadFailed>().is_some());
        assert!(error.callback::<io::Error>().is_none());
        assert!(Error::Conflict.callback::<LoadFailed>().is_none());
        assert_eq!(error.to_string(), "callback failed: load failed");
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn timeout_attribution() {
        let timeout = Error::Io(io::Error::from(io::ErrorKind::TimedOut)).for_key(b"k");
        assert!(matches!(timeout, Error::Timeout { ref key } if key == b"k"));
        let reset = Error::Io(io::Error::from(io::ErrorKind::ConnectionReset)).for_key(b"k");
        assert!(matches!(reset, Error::Io(_)));
    }

    #[test]
    fn duplicate_keeps_shape() {
        let source = Error::Ambiguous {
            key: b"k".to_vec(),
            source: Box::new(Error::Io(io::Error::new(io::ErrorKind::BrokenPipe, "pipe"))),
        };
        let copy = source.duplicate();
        assert!(copy.is_ambiguous());
        assert_eq!(copy.to_string(), source.to_string());
        let encode = Error::Encode(EncodeError::new("bad")).duplicate();
        assert!(matches!(encode, Error::Encode(_)));
        assert_eq!(encode.to_string(), "encode failed: bad");
    }

    #[test]
    fn display_renders_text_keys() {
        let error = Error::Timeout {
            key: b"user:1".to_vec(),
        };
        assert_eq!(error.to_string(), "timed out before sending request for \"user:1\"");
    }
}
