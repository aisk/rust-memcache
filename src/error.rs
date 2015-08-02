use std::error::Error;
use std::fmt;
use ffi::memcached_return_t;

#[derive(Debug)]
pub struct MemcacheError {
    pub code: memcached_return_t,
}

impl fmt::Display for MemcacheError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Memcache Error: {:?}", self.code)
    }
}

impl Error for MemcacheError {
    fn description(&self) -> &str{
        "TODO: description for this error"
    }

    fn cause(&self) -> Option<&Error> { None }
}

impl MemcacheError {
    pub fn new(code: memcached_return_t) -> MemcacheError {
        return MemcacheError{ code: code };
    }
}

pub type MemcacheResult<T> = Result<T, MemcacheError>;
