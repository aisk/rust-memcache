use std::io;
use std::fmt;
use std::error;
use std::string;

/// Stands for errors raised from rust-memcache
#[derive(Debug)]
pub enum MemcacheError {
    /// `std::io` related errors.
    Io(io::Error),
    /// Error raised when unserialize value data which from memcached to String
    FromUtf8(string::FromUtf8Error),
    /// Unknown error raised by memcached, [more detail](https://github.com/memcached/memcached/blob/master/doc/protocol.txt#L99-L101).
    Error,
    /// Client side error raised by memcached, probably caused by invalid input, [more detail](https://github.com/memcached/memcached/blob/master/doc/protocol.txt#L103-L107).
    ClientError(String),
    /// Server side error raise by memcached, [more detail](https://github.com/memcached/memcached/blob/master/doc/protocol.txt#L109-L116).
    ServerError(String),
    BinaryServerError(u16),
}

impl fmt::Display for MemcacheError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            MemcacheError::Io(ref err) => err.fmt(f),
            MemcacheError::FromUtf8(ref err) => err.fmt(f),
            MemcacheError::Error => write!(f, "Error"),
            MemcacheError::ClientError(ref s) => s.fmt(f),
            MemcacheError::ServerError(ref s) => s.fmt(f),
            MemcacheError::BinaryServerError(r) => write!(f, "BinaryServerError: {}", r),
        }
    }
}

impl error::Error for MemcacheError {
    fn description(&self) -> &str {
        match *self {
            MemcacheError::Io(ref err) => err.description(),
            MemcacheError::FromUtf8(ref err) => err.description(),
            MemcacheError::Error => "Error",
            MemcacheError::ClientError(ref s) => s.as_str(),
            MemcacheError::ServerError(ref s) => s.as_str(),
            MemcacheError::BinaryServerError(_) => "BinaryServerError",
        }
    }

    fn cause(&self) -> Option<&error::Error> {
        match *self {
            MemcacheError::Io(ref err) => err.cause(),
            MemcacheError::FromUtf8(ref err) => err.cause(),
            MemcacheError::Error => None,
            MemcacheError::ClientError(_) => None,
            MemcacheError::ServerError(_) => None,
            MemcacheError::BinaryServerError(_) => None,
        }
    }
}

impl From<io::Error> for MemcacheError {
    fn from(err: io::Error) -> MemcacheError {
        MemcacheError::Io(err)
    }
}

impl From<string::FromUtf8Error> for MemcacheError {
    fn from(err: string::FromUtf8Error) -> MemcacheError {
        MemcacheError::FromUtf8(err)
    }
}

impl From<String> for MemcacheError {
    fn from(s: String) -> MemcacheError {
        if s == "ERROR\r\n" {
            return MemcacheError::Error;
        } else if s.starts_with("CLIENT_ERROR") {
            return MemcacheError::ClientError(s);
        } else if s.starts_with("SERVER_ERROR") {
            return MemcacheError::ServerError(s);
        } else {
            panic!("{} if not a memcached error!", s);
        }
    }
}

impl From<u16> for MemcacheError {
    fn from(code: u16) -> MemcacheError {
        return MemcacheError::BinaryServerError(code);
    }
}
