use std::collections::HashMap;
use std::io::Write;
use byteorder::{WriteBytesExt, BigEndian};
use client::Stats;
use connection::Connection;
use error::MemcacheError;
use packet::{Opcode, PacketHeader, Magic};
use packet;
use protocol::Protocol;
use value::{ToMemcacheValue, FromMemcacheValue};

pub(crate) struct BinaryProtocol<'a> {
    connection: &'a mut Connection
}

impl<'a> BinaryProtocol<'a> {
    fn store<V: ToMemcacheValue<Connection>>(
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
        request_header.write(self.connection)?;
        self.connection.write_u32::<BigEndian>(extras.flags)?;
        self.connection.write_u32::<BigEndian>(extras.expiration)?;
        self.connection.write_all(key.as_bytes())?;
        value.write_to(self.connection)?;
        self.connection.flush()?;
        return packet::parse_header_only_response(self.connection);
    }
}

impl<'a> Protocol for BinaryProtocol<'a> {
    fn version(&mut self) -> Result<String, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Version as u8,
            ..Default::default()
        };
        request_header.write(self.connection)?;
        self.connection.flush()?;
        let version = packet::parse_version_response(self.connection)?;
        return Ok(version);
    }

   fn flush(&mut self) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Flush as u8,
            ..Default::default()
        };
        request_header.write(self.connection)?;
        self.connection.flush()?;
        packet::parse_header_only_response(self.connection)?;
        return Ok(());
    }

    fn flush_with_delay(&mut self, delay: u32) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Flush as u8,
            extras_length: 4,
            total_body_length: 4,
            ..Default::default()
        };
        request_header.write(self.connection)?;
        self.connection.write_u32::<BigEndian>(delay)?;
        self.connection.flush()?;
        packet::parse_header_only_response(self.connection)?;
        return Ok(());
    }

    fn get<V: FromMemcacheValue>(&mut self, key: &str) -> Result<Option<V>, MemcacheError> {
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
        request_header.write(self.connection)?;
        self.connection.write_all(key.as_bytes())?;
        self.connection.flush()?;
        return packet::parse_get_response(self.connection);
    }

    fn gets<V: FromMemcacheValue>(
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
            request_header.write(self.connection)?;
            self.connection.write_all(key.as_bytes())?;
        }
        let noop_request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Noop as u8,
            ..Default::default()
        };
        noop_request_header.write(self.connection)?;
        return packet::parse_gets_response(self.connection);
    }

    fn set<V: ToMemcacheValue<Connection>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Set, key, value, expiration);
    }

    fn add<V: ToMemcacheValue<Connection>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Add, key, value, expiration);
    }

    fn replace<V: ToMemcacheValue<Connection>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Replace, key, value, expiration);
    }

    fn append<V: ToMemcacheValue<Connection>>(
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
        request_header.write(self.connection)?;
        self.connection.write_all(key.as_bytes())?;
        value.write_to(self.connection)?;
        self.connection.flush()?;
        return packet::parse_header_only_response(self.connection);
    }

    fn prepend<V: ToMemcacheValue<Connection>>(
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
        request_header.write(self.connection)?;
        self.connection.write_all(key.as_bytes())?;
        value.write_to(&mut self.connection)?;
        self.connection.flush()?;
        return packet::parse_header_only_response(self.connection);
    }

    fn delete(&mut self, key: &str) -> Result<bool, MemcacheError> {
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
        request_header.write(self.connection)?;
        self.connection.write_all(key.as_bytes())?;
        self.connection.flush()?;
        return packet::parse_delete_response(self.connection);
    }

    fn increment(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
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
        request_header.write(self.connection)?;
        self.connection.write_u64::<BigEndian>(
            extras.amount,
        )?;
        self.connection.write_u64::<BigEndian>(
            extras.initial_value,
        )?;
        self.connection.write_u32::<BigEndian>(
            extras.expiration,
        )?;
        self.connection.write_all(key.as_bytes())?;
        self.connection.flush()?;
        return packet::parse_counter_response(self.connection);
    }

    fn decrement(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
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
        request_header.write(self.connection)?;
        self.connection.write_u64::<BigEndian>(
            extras.amount,
        )?;
        self.connection.write_u64::<BigEndian>(
            extras.initial_value,
        )?;
        self.connection.write_u32::<BigEndian>(
            extras.expiration,
        )?;
        self.connection.write_all(key.as_bytes())?;
        self.connection.flush()?;
        return packet::parse_counter_response(self.connection);
    }

    fn touch(&mut self, key: &str, expiration: u32) -> Result<bool, MemcacheError> {
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
        request_header.write(self.connection)?;
        self.connection.write_u32::<BigEndian>(expiration)?;
        self.connection.write_all(key.as_bytes())?;
        self.connection.flush()?;
        return packet::parse_touch_response(self.connection);
    }

    fn stats(&mut self) -> Result<Stats, MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Stat as u8,
            ..Default::default()
        };
        request_header.write(self.connection)?;
        self.connection.flush()?;
        let stats_info = packet::parse_stats_response(self.connection)?;
        return Ok(stats_info);
    }
}
