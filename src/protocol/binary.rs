use std::collections::HashMap;
use std::io::Write;

use super::check_key_len;
use byteorder::{BigEndian, WriteBytesExt};
use client::Stats;
use error::MemcacheError;
use protocol::binary_packet::{self, Magic, Opcode, PacketHeader};
use stream::Stream;
use value::{FromMemcacheValueExt, ToMemcacheValue};

pub struct BinaryProtocol {
    pub stream: Stream,
}

impl BinaryProtocol {
    pub(super) fn auth(&mut self, username: &str, password: &str) -> Result<(), MemcacheError> {
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

    fn send_request<V: ToMemcacheValue<Stream>>(
        &mut self,
        opcode: Opcode,
        key: &str,
        value: V,
        expiration: u32,
        cas: Option<u64>,
    ) -> Result<(), MemcacheError> {
        check_key_len(key)?;
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
        Ok(())
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
        self.stream.flush()?;
        binary_packet::parse_response(&mut self.stream)?.err().map(|_| ())
    }

    /// Support efficient multi-store operations using pipelining.
    fn stores<V: ToMemcacheValue<Stream>, K: AsRef<str>, I: IntoIterator<Item = (K, V)>>(
        &mut self,
        opcode: Opcode,
        entries: I,
        expiration: u32,
        cas: Option<u64>,
    ) -> Result<(), MemcacheError> {
        let mut sent_count = 0;
        for (key, value) in entries {
            self.send_request(opcode, key.as_ref(), value, expiration, cas)?;
            sent_count += 1;
        }
        // Flush now that all the requests have been written.
        self.stream.flush()?;

        // Receive all the responses. If there were errors, return the first.
        let mut final_result = Ok(());

        for _ in 0..sent_count {
            match binary_packet::parse_response(&mut self.stream) {
                Ok(_) => (),
                Err(e) if e.is_recoverable() => {
                    // Recoverable error. Report it after reading the rest of the responses.
                    if final_result.is_ok() {
                        final_result = Err(e);
                    }
                }
                Err(e) => return Err(e), // Unrecoverable error. Stop immediately.
            };
        }

        final_result
    }

    pub(super) fn version(&mut self) -> Result<String, MemcacheError> {
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

    pub(super) fn flush(&mut self) -> Result<(), MemcacheError> {
        let request_header = PacketHeader {
            magic: Magic::Request as u8,
            opcode: Opcode::Flush as u8,
            ..Default::default()
        };
        request_header.write(&mut self.stream)?;
        self.stream.flush()?;
        binary_packet::parse_response(&mut self.stream)?.err().map(|_| ())
    }

    pub(super) fn flush_with_delay(&mut self, delay: u32) -> Result<(), MemcacheError> {
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

    pub(super) fn get<V: FromMemcacheValueExt>(&mut self, key: &str) -> Result<Option<V>, MemcacheError> {
        check_key_len(key)?;
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

    pub(super) fn gets<V: FromMemcacheValueExt, K: AsRef<str>>(
        &mut self,
        keys: &[K],
    ) -> Result<HashMap<String, V>, MemcacheError> {
        for k in keys.iter() {
            check_key_len(k.as_ref())?;
        }

        for k in keys.iter() {
            let key = k.as_ref();
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
        self.stream.flush()?;
        return binary_packet::parse_gets_response(&mut self.stream, keys.len());
    }

    pub(super) fn cas<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
        cas: u64,
    ) -> Result<bool, MemcacheError> {
        self.send_request(Opcode::Set, key, value, expiration, Some(cas))?;
        self.stream.flush()?;
        binary_packet::parse_cas_response(&mut self.stream)
    }

    pub(super) fn set<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Set, key, value, expiration, None);
    }

    pub(super) fn sets<V: ToMemcacheValue<Stream>, K: AsRef<str>, I: IntoIterator<Item = (K, V)>>(
        &mut self,
        entries: I,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.stores(Opcode::Set, entries, expiration, None);
    }

    pub(super) fn add<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Add, key, value, expiration, None);
    }

    pub(super) fn replace<V: ToMemcacheValue<Stream>>(
        &mut self,
        key: &str,
        value: V,
        expiration: u32,
    ) -> Result<(), MemcacheError> {
        return self.store(Opcode::Replace, key, value, expiration, None);
    }

    pub(super) fn append<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError> {
        check_key_len(key)?;
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

    pub(super) fn prepend<V: ToMemcacheValue<Stream>>(&mut self, key: &str, value: V) -> Result<(), MemcacheError> {
        check_key_len(key)?;
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

    pub(super) fn delete(&mut self, key: &str) -> Result<bool, MemcacheError> {
        Ok(self.deletes(&[key])?[0])
    }

    pub(super) fn deletes<K: AsRef<str>>(&mut self, keys: &[K]) -> Result<Vec<bool>, MemcacheError> {
        for k in keys.iter() {
            check_key_len(k.as_ref())?;
        }

        for k in keys.iter() {
            let key = k.as_ref();
            let request_header = PacketHeader {
                magic: Magic::Request as u8,
                opcode: Opcode::Delete as u8,
                key_length: key.len() as u16,
                total_body_length: key.len() as u32,
                ..Default::default()
            };
            request_header.write(&mut self.stream)?;
            self.stream.write_all(key.as_bytes())?;
        }
        // Flush now that all the requests have been written.
        self.stream.flush()?;

        // Receive all the responses. If there were errors, return the first.
        let mut final_result = Ok(Vec::with_capacity(keys.len()));

        for _ in 0..keys.len() {
            match binary_packet::parse_delete_response(&mut self.stream) {
                Ok(deleted) => {
                    if let Ok(deleted_list) = &mut final_result {
                        deleted_list.push(deleted);
                    }
                }
                Err(e) if e.is_recoverable() => {
                    // Recoverable error. Report it after reading the rest of the responses.
                    if final_result.is_ok() {
                        final_result = Err(e);
                    }
                }
                Err(e) => return Err(e), // Unrecoverable error. Stop immediately.
            }
        }

        final_result
    }

    pub(super) fn increment(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        check_key_len(key)?;
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

    pub(super) fn decrement(&mut self, key: &str, amount: u64) -> Result<u64, MemcacheError> {
        check_key_len(key)?;
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

    pub(super) fn touch(&mut self, key: &str, expiration: u32) -> Result<bool, MemcacheError> {
        check_key_len(key)?;
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

    pub(super) fn stats(&mut self) -> Result<Stats, MemcacheError> {
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
