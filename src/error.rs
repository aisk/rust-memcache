use std::error::Error;
use std::fmt;
use std::convert::From;
use memcached_sys::memcached_return_t;

#[derive(Debug)]
pub struct LibMemcachedError {
    pub code: memcached_return_t,
}

impl fmt::Display for LibMemcachedError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Memcache Error: {:?}", self.code)
    }
}

impl Error for LibMemcachedError {
    fn description(&self) -> &str{
        "TODO: description for this error"
    }

    fn cause(&self) -> Option<&Error> { None }
}

impl LibMemcachedError {
    pub fn new(code: memcached_return_t) -> LibMemcachedError {
        return LibMemcachedError{ code: code };
    }
}

#[derive(Debug)]
pub enum MemcacheError {
    LibMemcached(LibMemcachedError),
}

impl From<LibMemcachedError> for MemcacheError {
    fn from(err: LibMemcachedError) -> MemcacheError {
        return MemcacheError::LibMemcached(err);
    }
}

#[must_use]
pub type MemcacheResult<T> = Result<T, MemcacheError>;
