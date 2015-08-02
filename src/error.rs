use ffi::memcached_return_t;

#[derive(Debug)]
pub struct MemcacheError {
    pub kind: memcached_return_t,
}

pub type MemcacheResult<T> = Result<T, MemcacheError>;
