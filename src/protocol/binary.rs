use std::collections::HashMap;
use std::io::Write;
use byteorder::{WriteBytesExt, BigEndian};
use client::Stats;
use stream::Stream;
use error::MemcacheError;
use packet::{Opcode, PacketHeader, Magic};
use packet;
use value::{ToMemcacheValue, FromMemcacheValue};

pub struct BinaryProtocol {
    pub stream: Stream,
}

impl BinaryProtocol {
    fn store<V: ToMemcacheValue<Stream>>(
        &mut self,
        opcode: Opcode,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        if key.len() > 250 {
            return Err(MemcacheError::ClientError(String::from("key is too long")));
        }
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: opcode as u8,
            key_length: key.len() as u16,
            extras_length: 8,
            total_body_length: (8 + key.len() + value.get_length()) as u32,
            ..Default::default()
        };
        let extras = packet::StoreExtras {
            flags: value.get_flags(),
            expiration,
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_u32::<BigEndian>(extras.flags)?;
        self.stream.write_u32::<BigEndian>(extras.expiration)?;
        self.stream.write_all(key.as_bytes())?;
        value.write_to(&mut self.stream)?;
        self.stream.flush()?;
        return packet::parse_header_only_response(&mut self.stream);
    }

    pub fn version(&mut self) -> Result<String, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Version as u8,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.flush()?;
        let version = packet::parse_version_response(&mut self.stream)?;
        return Ok(version);
    }

   pub fn flush(&mut self) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Flush as u8,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.flush()?;
        packet::parse_header_only_response(&mut self.stream)?;
        return Ok(());
    }

    pub fn flush_with_delay(&mut self, delay: u32) -> Result<(), MemcacheError> {
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
        packet::parse_header_only_response(&mut self.stream)?;
        return Ok(());
    }

    pub fn get<V: FromMemcacheValue>(&mut self, key: &str) -> Result<Option<V>, MemcacheError> {
        if key.len() > 250 {
            return Err(MemcacheError::ClientError(String::from("key is too long")));
        }
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
        return packet::parse_get_response(&mut self.stream);
    }

    pub fn gets<V: FromMemcacheValue>(
        &mut self,
        keys: Vec<&str>,
    ) -> Result<HashMap<String, V>, MemcacheError> {
        for key in keys {
            if key.len() > 250 {
                return Err(MemcacheError::ClientError(String::from("key is too long")));
            }
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
        return packet::parse_gets_response(&mut self.stream);
    }

    pub fn set<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Set, key, value, expiration);
    }

    pub fn add<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Add, key, value, expiration);
    }

    pub fn replace<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Replace, key, value, expiration);
    }

    pub fn append<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
    ) -> Result<(), MemcacheError> {
        if key.len() > 250 {
            return Err(MemcacheError::ClientError(String::from("key is too long")));
        }
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
        return packet::parse_header_only_response(&mut self.stream);
    }

    pub fn prepend<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
    ) -> Result<(), MemcacheError> {
        if key.len() > 250 {
            return Err(MemcacheError::ClientError(String::from("key is too long")));
        }
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
        return packet::parse_header_only_response(&mut self.stream);
    }

    pub fn delete(&mut self, key: &str) -> Result<bool, MemcacheError> {
        if key.len() > 250 {
            return Err(MemcacheError::ClientError(String::from("key is too long")));
        }
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
        return packet::parse_delete_response(&mut self.stream);
    }

    pub fn increment(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        if key.len() > 250 {
            return Err(MemcacheError::ClientError(String::from("key is too long")));
        }
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Increment as u8,
            key_length: key.len() as u16,
            extras_length: 20,
            total_body_length: (20 + key.len()) as u32,
            ..Default::default()
        };
        let extras = packet::CounterExtras {
            amount,
            initial_value: 0,
            expiration: 0,
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_u64::<BigEndian>(
            extras.amount,
        )?;
        self.stream.write_u64::<BigEndian>(
            extras.initial_value,
        )?;
        self.stream.write_u32::<BigEndian>(
            extras.expiration,
        )?;
        self.stream.write_all(key.as_bytes())?;
        self.stream.flush()?;
        return packet::parse_counter_response(&mut self.stream);
    }

    pub fn decrement(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        if key.len() > 250 {
            return Err(MemcacheError::ClientError(String::from("key is too long")));
        }
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Decrement as u8,
            key_length: key.len() as u16,
            extras_length: 20,
            total_body_length: (20 + key.len()) as u32,
            ..Default::default()
        };
        let extras = packet::CounterExtras {
            amount,
            initial_value: 0,
            expiration: 0,
        };
        request_header.write(&mut self.stream)?;
        self.stream.write_u64::<BigEndian>(
            extras.amount,
        )?;
        self.stream.write_u64::<BigEndian>(
            extras.initial_value,
        )?;
        self.stream.write_u32::<BigEndian>(
            extras.expiration,
        )?;
        self.stream.write_all(key.as_bytes())?;
        self.stream.flush()?;
        return packet::parse_counter_response(&mut self.stream);
    }

    pub fn touch(&mut self, key: &str, expiration: u32) -> Result<bool, MemcacheError> {
        if key.len() > 250 {
            return Err(MemcacheError::ClientError(String::from("key is too long")));
        }
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
        return packet::parse_touch_response(&mut self.stream);
    }

    pub fn stats(&mut self) -> Result<Stats, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Stat as u8,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.flush()?;
        let stats_info = packet::parse_stats_response(&mut self.stream)?;
        return Ok(stats_info);
    }
}
