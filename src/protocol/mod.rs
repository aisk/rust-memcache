mod binary;
mod ascii;

use std::collections::HashMap;
use client::Stats;
use connection::Connection;
use error::MemcacheError;
use value::{ToMemcacheValue, FromMemcacheValue};

pub(crate) trait Protocol{
    fn version(&mut self) -> Result<String, MemcacheError>;
    fn flush(&mut self) -> Result<(), MemcacheError>;
    fn flush_with_delay(&mut self, delay: u32) -> Result<(), MemcacheError>;
    fn get<V: FromMemcacheValue>(&mut self, key: &str) -> Result<Option<V>, MemcacheError>;
    fn gets<V: FromMemcacheValue>(&mut self, keys: Vec<&str>) -> Result<HashMap<String, V>, MemcacheError>;
    fn set<V: ToMemcacheValue<Connection>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError>;
    fn add<V: ToMemcacheValue<Connection>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError>;
    fn replace<V: ToMemcacheValue<Connection>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError>;
    fn append<V: ToMemcacheValue<Connection>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError>;
    fn prepend<V: ToMemcacheValue<Connection>>(&mut self, key: &str,value: V) -> Result<(), MemcacheError>;
    fn delete(&mut self, key: &str) -> Result<bool, MemcacheError>;
    fn increment(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError>;
    fn decrement(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError>;
    fn touch(&mut self, key: &str, expiration: u32) -> Result<bool, MemcacheError>;
    fn stats(&mut self) -> Result<Stats, MemcacheError>;
}
