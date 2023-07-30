use std::collections::HashMap;
use std::io::Write;

use super::ProtocolTrait;
use crate::client::Stats;
use crate::error::MemcacheError;
use crate::protocol::binary_packet::{self, Magic, Opcode, PacketHeader};
use crate::stream::Stream;
use crate::value::{FromMemcacheValueExt, ToMemcacheValue};
use byteorder::{BigEndian, WriteBytesExt};

pub struct BinaryProtocol {
    pub stream: Stream,
}

impl ProtocolTrait for BinaryProtocol {
    fn auth(&mut self, username: &str, password: &str) -> Result<(), MemcacheError> {
        let key = "PLAIN";
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::StartAuth as u8,
            key_length: key.len() as u16,
            total_body_length: (key.len() + username.len() + password.len() + 2) as u32,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_all(key.as_bytes())?;
        write!(&mut self.stream, "\x00{}\x00{}", username, password)?;
        self.stream.flush()?;
        binary_packet::parse_start_auth_response(&mut self.stream).map(|_| ())
    }

    fn version(&mut self) -> Result<String, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Version as u8,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.flush()?;
        let version = binary_packet::parse_version_response(&mut self.stream)?;
        return Ok(version);
    }

    fn flush(&mut self) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Flush as u8,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.flush()?;
        binary_packet::parse_response(&mut self.stream)?.err().map(|_| ())
    }

    fn flush_with_delay(&mut self, delay: u32) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Flush as u8,
            extras_length: 4,
            total_body_length: 4,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_u32::<BigEndian>(delay)?;
        self.stream.flush()?;
        binary_packet::parse_response(&mut self.stream)?.err().map(|_| ())
    }

    fn get<V: FromMemcacheValueExt>(&mut self, key: &str) -> Result<Option<V>, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Get as u8,
            key_length: key.len() as u16,
            total_body_length: key.len() as u32,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_all(key.as_bytes())?;
        self.stream.flush()?;
        return binary_packet::parse_get_response(&mut self.stream);
    }

    fn gets<V: FromMemcacheValueExt>(&mut self, keys: &[&str]) -> Result<HashMap<String, V>, MemcacheError> {
        for key in keys {
            let request_header = PacketHeader {
                magic: Magic::Request as u8,
                opcode: Opcode::GetKQ as u8,
                key_length: key.len() as u16,
                total_body_length: key.len() as u32,
                ..Default::default()
            };
            request_header.write(&mut self.stream)?;
            self.stream.write_all(key.as_bytes())?;
        }
        let noop_request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Noop as u8,
            ..Default::default()
        };
        noop_request_header.write(&mut self.stream)?;
        return binary_packet::parse_gets_response(&mut self.stream, keys.len());
    }

    fn cas<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
        cas: u64,
    ) -> Result<bool, MemcacheError> {
        self.send_request(Opcode::Set, key, value, expiration, Some(cas))?;
        binary_packet::parse_cas_response(&mut self.stream)
    }

    fn set<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError> {
        return self.store(Opcode::Set, key, value, expiration, None);
    }

    fn add<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V, expiration: u32) -> Result<(), MemcacheError> {
        return self.store(Opcode::Add, key, value, expiration, None);
    }

    fn replace<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Replace, key, value, expiration, None);
    }

    fn append<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Append as u8,
            key_length: key.len() as u16,
            total_body_length: (key.len() + value.get_length()) as u32,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_all(key.as_bytes())?;
        value.write_to(&mut self.stream)?;
        self.stream.flush()?;
        binary_packet::parse_response(&mut self.stream)?.err().map(|_| ())
    }

    fn prepend<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Prepend as u8,
            key_length: key.len() as u16,
            total_body_length: (key.len() + value.get_length()) as u32,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_all(key.as_bytes())?;
        value.write_to(&mut self.stream)?;
        self.stream.flush()?;
        binary_packet::parse_response(&mut self.stream).map(|_| ())
    }

    fn delete(&mut self, key: &str) -> Result<bool, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Delete as u8,
            key_length: key.len() as u16,
            total_body_length: key.len() as u32,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_all(key.as_bytes())?;
        self.stream.flush()?;
        return binary_packet::parse_delete_response(&mut self.stream);
    }

    fn increment(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Increment as u8,
            key_length: key.len() as u16,
            extras_length: 20,
            total_body_length: (20 + key.len()) as u32,
            ..Default::default()
        };
        let extras = binary_packet::CounterExtras {
            amount,
            initial_value: 0,
            expiration: 0,
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_u64::<BigEndian>(extras.amount)?;
        self.stream.write_u64::<BigEndian>(extras.initial_value)?;
        self.stream.write_u32::<BigEndian>(extras.expiration)?;
        self.stream.write_all(key.as_bytes())?;
        self.stream.flush()?;
        return binary_packet::parse_counter_response(&mut self.stream);
    }

    fn decrement(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Decrement as u8,
            key_length: key.len() as u16,
            extras_length: 20,
            total_body_length: (20 + key.len()) as u32,
            ..Default::default()
        };
        let extras = binary_packet::CounterExtras {
            amount,
            initial_value: 0,
            expiration: 0,
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_u64::<BigEndian>(extras.amount)?;
        self.stream.write_u64::<BigEndian>(extras.initial_value)?;
        self.stream.write_u32::<BigEndian>(extras.expiration)?;
        self.stream.write_all(key.as_bytes())?;
        self.stream.flush()?;
        return binary_packet::parse_counter_response(&mut self.stream);
    }

    fn touch(&mut self, key: &str, expiration: u32) -> Result<bool, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Touch as u8,
            key_length: key.len() as u16,
            extras_length: 4,
            total_body_length: (key.len() as u32 + 4),
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_u32::<BigEndian>(expiration)?;
        self.stream.write_all(key.as_bytes())?;
        self.stream.flush()?;
        return binary_packet::parse_touch_response(&mut self.stream);
    }

    fn stats(&mut self) -> Result<Stats, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Stat as u8,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.flush()?;
        let stats_info = binary_packet::parse_stats_response(&mut self.stream)?;
        return Ok(stats_info);
    }
}

impl BinaryProtocol {
    fn send_request<V: ToMemcacheValue<Stream>>(
        &mut self,
        opcode: Opcode,
        key: &str,
        value: V,
        expiration: u32,
        cas: Option<u64>,
    ) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: opcode as u8,
            key_length: key.len() as u16,
            extras_length: 8,
            total_body_length: (8 + key.len() + value.get_length()) as u32,
            cas: cas.unwrap_or(0),
            ..Default::default()
        };
        let extras = binary_packet::StoreExtras {
            flags: value.get_flags(),
            expiration,
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_u32::<BigEndian>(extras.flags)?;
        self.stream.write_u32::<BigEndian>(extras.expiration)?;
        self.stream.write_all(key.as_bytes())?;
        value.write_to(&mut self.stream)?;
        self.stream.flush().map_err(Into::into)
    }

    fn store<V: ToMemcacheValue<Stream>>(
        &mut self,
        opcode: Opcode,
        key: &str,
        value: V,
        expiration: u32,
        cas: Option<u64>,
    ) -> Result<(), MemcacheError> {
        self.send_request(opcode, key, value, expiration, cas)?;
        binary_packet::parse_response(&mut self.stream)?.err().map(|_| ())
    }
}
