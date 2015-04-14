#![crate_name = "memcache"]
#![crate_type = "rlib"]

extern crate hash_ring;

use std::io::Error;
use std::convert::From;

#[derive(Debug)]
pub enum MemcacheError {
    InternalIoError(Error),
    ServerError
}

impl From<Error> for MemcacheError {
    fn from(err: Error) -> MemcacheError {
        MemcacheError::InternalIoError(err)
    }
}

pub type MemcacheResult<T> = Result<T, MemcacheError>;

// Trait defining Memcache protocol both for multi server connection using `Client` as
// well as single connection using `Connection`.
trait Commands {
    fn flush(&mut self) -> MemcacheResult<()>;
    fn delete(&mut self, key: &str) -> MemcacheResult<bool>;
    fn get(&mut self, key: &str) -> MemcacheResult<Option<(Vec<u8>, u16)>>;
    fn set(&mut self, key: &str, value: &[u8], exptime: isize, flags: u16) -> MemcacheResult<bool>;
    fn incr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>>;
    fn decr(&mut self, key: &str, value: u64) -> MemcacheResult<Option<(isize)>>;
}

mod connection;
mod client;
