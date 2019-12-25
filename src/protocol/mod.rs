mod ascii;
mod binary;
mod binary_packet;

use client::Stats;
use enum_dispatch::enum_dispatch;
use error::MemcacheError;
pub(crate) use protocol::ascii::AsciiProtocol;
pub(crate) use protocol::binary::BinaryProtocol;
use std::collections::HashMap;
use stream::Stream;
use value::{FromMemcacheValueExt, ToMemcacheValue};

#[enum_dispatch]
pub enum Protocol {
    Ascii(AsciiProtocol<Stream>),
    Binary(BinaryProtocol),
}

#[allow(dead_code)]
pub enum ResponseStatus {
    NoError = 0x00,
    KeyNotFound = 0x01,
    KeyExists = 0x02,
    ValueTooLarge = 0x03,
    InvalidArguments = 0x04,
    AuthenticationRequired = 0x20,
}

#[enum_dispatch(Protocol)]
pub trait ProtocolTrait {
    fn auth(&mut self, username: &str, password: &str) -> Result<(), MemcacheError>;
    fn version(&mut self) -> Result<String, MemcacheError>;
    fn flush(&mut self) -> Result<(), MemcacheError>;
    fn flush_with_delay(&mut self, delay: u32) -> Result<(), MemcacheError>;
    fn get<V: FromMemcacheValueExt>(&mut self, key: &str) -> Result<Option<V>, MemcacheError>;
    fn gets<V: FromMemcacheValueExt>(&mut self, keys: Vec<&str>) -> Result<HashMap<String, V>, MemcacheError>;
    fn set<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError>;
    fn cas<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
        cas: u64,
    ) -> Result<bool, MemcacheError>;
    fn add<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError>;
    fn replace<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError>;
    fn append<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError>;
    fn prepend<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError>;
    fn delete(&mut self, key: &str) -> Result<bool, MemcacheError>;
    fn increment(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError>;
    fn decrement(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError>;
    fn touch(&mut self, key: &str, expiration: u32) -> Result<bool, MemcacheError>;
    fn stats(&mut self) -> Result<Stats, MemcacheError>;
}
