mod binary;
mod ascii;

use std::collections::HashMap;
use client::Stats;
use stream::Stream;
use error::MemcacheError;
use value::{ToMemcacheValue, FromMemcacheValue};
pub(crate) use protocol::binary::BinaryProtocol;

pub enum Protocol {
    Binary(BinaryProtocol),
}

impl Protocol {
    pub fn version(&mut self) -> Result<String, MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.version()
        }
    }

    pub fn flush(&mut self) -> Result<(), MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.flush()
        }
    }

    pub fn flush_with_delay(&mut self, delay: u32) -> Result<(), MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.flush_with_delay(delay)
        }
    }

    pub fn get<V: FromMemcacheValue>(&mut self, key: &str) -> Result<Option<V>, MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.get(key)
        }
    }

    pub fn gets<V: FromMemcacheValue>(&mut self, keys: Vec<&str>) -> Result<HashMap<String, V>, MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.gets(keys)
        }
    }

    pub fn set<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.set(key, value, expiration)
        }
    }

    pub fn add<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.add(key, value, expiration)
        }
    }

    pub fn replace<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.replace(key, value, expiration)
        }
    }

    pub fn append<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.append(key, value)
        }
    }

    pub fn prepend<V: ToMemcacheValue<Stream>>(&mut self, key: &str,value: V) -> Result<(), MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.prepend(key, value)
        }
    }

    pub fn delete(&mut self, key: &str) -> Result<bool, MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.delete(key)
        }
    }

    pub fn increment(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.increment(key, amount)
        }
    }

    pub fn decrement(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.decrement(key, amount)
        }
    }

    pub fn touch(&mut self, key: &str, expiration: u32) -> Result<bool, MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.touch(key, expiration)
        }
    }

    pub fn stats(&mut self) -> Result<Stats, MemcacheError> {
        match self {
            Protocol::Binary(ref mut protocol) => protocol.stats()
        }
    }
}
