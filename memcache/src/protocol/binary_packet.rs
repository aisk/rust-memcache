use crate::error::{CommandError, MemcacheError, ServerError};
use crate::value::FromMemcacheValueExt;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{self, Cursor};

const OK_STATUS: u16 = 0x0;

#[allow(dead_code)]
pub enum Opcode {
    Get = 0x00,
    Set = 0x01,
    Add = 0x02,
    Replace = 0x03,
    Delete = 0x04,
    Increment = 0x05,
    Decrement = 0x06,
    Flush = 0x08,
    Stat = 0x10,
    Noop = 0x0a,
    Version = 0x0b,
    GetKQ = 0x0d,
    Append = 0x0e,
    Prepend = 0x0f,
    Touch = 0x1c,
    StartAuth = 0x21,
}

pub enum Magic {
    Request = 0x80,
    Response = 0x81,
}

#[derive(Debug, Default)]
pub struct PacketHeader {
    pub magic: u8,
    pub opcode: u8,
    pub key_length: u16,
    pub extras_length: u8,
    pub data_type: u8,
    pub vbucket_id_or_status: u16,
    pub total_body_length: u32,
    pub opaque: u32,
    pub cas: u64,
}

#[derive(Debug)]
pub struct StoreExtras {
    pub flags: u32,
    pub expiration: u32,
}

#[derive(Debug)]
pub struct CounterExtras {
    pub amount: u64,
    pub initial_value: u64,
    pub expiration: u32,
}

impl PacketHeader {
    pub fn write<W: io::Write>(self, writer: &mut W) -> Result<(), io::Error> {
        writer.write_u8(self.magic)?;
        writer.write_u8(self.opcode)?;
        writer.write_u16::<BigEndian>(self.key_length)?;
        writer.write_u8(self.extras_length)?;
        writer.write_u8(self.data_type)?;
        writer.write_u16::<BigEndian>(self.vbucket_id_or_status)?;
        writer.write_u32::<BigEndian>(self.total_body_length)?;
        writer.write_u32::<BigEndian>(self.opaque)?;
        writer.write_u64::<BigEndian>(self.cas)?;
        return Ok(());
    }

    pub fn read<R: io::Read>(reader: &mut R) -> Result<PacketHeader, MemcacheError> {
        let magic = reader.read_u8()?;
        if magic != Magic::Response as u8 {
            return Err(ServerError::BadMagic(magic).into());
        }
        let header = PacketHeader {
            magic,
            opcode: reader.read_u8()?,
            key_length: reader.read_u16::<BigEndian>()?,
            extras_length: reader.read_u8()?,
            data_type: reader.read_u8()?,
            vbucket_id_or_status: reader.read_u16::<BigEndian>()?,
            total_body_length: reader.read_u32::<BigEndian>()?,
            opaque: reader.read_u32::<BigEndian>()?,
            cas: reader.read_u64::<BigEndian>()?,
        };
        return Ok(header);
    }
}

pub struct Response {
    header: PacketHeader,
    key: Vec<u8>,
    extras: Vec<u8>,
    value: Vec<u8>,
}

impl Response {
    pub(crate) fn err(self) -> Result<Self, MemcacheError> {
        let status = self.header.vbucket_id_or_status;
        if status == OK_STATUS {
            Ok(self)
        } else {
            Err(CommandError::from(status))?
        }
    }
}

pub fn parse_response<R: io::Read>(reader: &mut R) -> Result<Response, MemcacheError> {
    let header = PacketHeader::read(reader)?;
    let mut extras = vec![0x0; header.extras_length as usize];
    reader.read_exact(extras.as_mut_slice())?;

    let mut key = vec![0x0; header.key_length as usize];
    reader.read_exact(key.as_mut_slice())?;

    // TODO: return error if total_body_length < extras_length + key_length
    let mut value =
        vec![0x0; (header.total_body_length - u32::from(header.key_length) - u32::from(header.extras_length)) as usize];
    reader.read_exact(value.as_mut_slice())?;

    Ok(Response {
        header,
        key,
        extras,
        value,
    })
}

pub fn parse_cas_response<R: io::Read>(reader: &mut R) -> Result<bool, MemcacheError> {
    match parse_response(reader)?.err() {
        Err(MemcacheError::CommandError(e)) if e == CommandError::KeyNotFound || e == CommandError::KeyExists => {
            Ok(false)
        }
        Ok(_) => Ok(true),
        Err(e) => Err(e),
    }
}

pub fn parse_version_response<R: io::Read>(reader: &mut R) -> Result<String, MemcacheError> {
    let Response { value, .. } = parse_response(reader)?.err()?;
    Ok(String::from_utf8(value)?)
}

pub fn parse_get_response<R: io::Read, V: FromMemcacheValueExt>(reader: &mut R) -> Result<Option<V>, MemcacheError> {
    match parse_response(reader)?.err() {
        Ok(Response {
            header, extras, value, ..
        }) => {
            let flags = Cursor::new(extras).read_u32::<BigEndian>()?;
            Ok(Some(FromMemcacheValueExt::from_memcache_value(
                value,
                flags,
                Some(header.cas),
            )?))
        }
        Err(MemcacheError::CommandError(CommandError::KeyNotFound)) => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn parse_gets_response<R: io::Read, V: FromMemcacheValueExt>(
    reader: &mut R,
    max_responses: usize,
) -> Result<HashMap<String, V>, MemcacheError> {
    let mut result = HashMap::new();
    for _ in 0..=max_responses {
        let Response {
            header,
            key,
            extras,
            value,
        } = parse_response(reader)?.err()?;
        if header.opcode == Opcode::Noop as u8 {
            return Ok(result);
        }
        let flags = Cursor::new(extras).read_u32::<BigEndian>()?;
        let key = String::from_utf8(key)?;
        result.insert(
            key,
            FromMemcacheValueExt::from_memcache_value(value, flags, Some(header.cas))?,
        );
    }
    Err(ServerError::BadResponse(Cow::Borrowed("Expected end of gets response")))?
}

pub fn parse_delete_response<R: io::Read>(reader: &mut R) -> Result<bool, MemcacheError> {
    match parse_response(reader)?.err() {
        Ok(_) => Ok(true),
        Err(MemcacheError::CommandError(CommandError::KeyNotFound)) => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn parse_counter_response<R: io::Read>(reader: &mut R) -> Result<u64, MemcacheError> {
    let Response { value, .. } = parse_response(reader)?.err()?;
    Ok(Cursor::new(value).read_u64::<BigEndian>()?)
}

pub fn parse_touch_response<R: io::Read>(reader: &mut R) -> Result<bool, MemcacheError> {
    match parse_response(reader)?.err() {
        Ok(_) => Ok(true),
        Err(MemcacheError::CommandError(CommandError::KeyNotFound)) => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn parse_stats_response<R: io::Read>(reader: &mut R) -> Result<HashMap<String, String>, MemcacheError> {
    let mut result = HashMap::new();
    loop {
        let Response { key, value, .. } = parse_response(reader)?.err()?;
        let key = String::from_utf8(key)?;
        let value = String::from_utf8(value)?;
        if key.is_empty() && value.is_empty() {
            break;
        }
        result.insert(key, value);
    }
    Ok(result)
}

pub fn parse_start_auth_response<R: io::Read>(reader: &mut R) -> Result<bool, MemcacheError> {
    parse_response(reader)?.err().map(|_| true)
}
