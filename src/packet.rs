use std::io;
use byteorder::{WriteBytesExt, ReadBytesExt, BigEndian};
use error::MemcacheError;
use value::FromMemcacheValue;

#[allow(dead_code)]
pub enum Opcode {
    Get = 0x00,
    Set = 0x01,
    Add = 0x02,
    Repalce = 0x03,
    Delete = 0x04,
    Flush = 0x08,
    Version = 0x0b,
}

pub enum Magic {
    Request = 0x80,
    Response = 0x81,
}

#[allow(dead_code)]
pub enum ResponseStatus {
    NoError = 0x0000,
    KeyNotFound = 0x0001,
    KeyExits = 0x0002,
    ValueTooLarge = 0x003,
    InvalidArguments = 0x004,
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

impl PacketHeader {
    pub fn write<W: io::Write>(self, mut writer: W) -> Result<(), io::Error> {
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

    pub fn read<R: io::Read>(mut reader: R) -> Result<PacketHeader, MemcacheError> {
        let magic = reader.read_u8()?;
        if magic != Magic::Response as u8 {
            return Err(MemcacheError::ClientError(
                String::from("Bad magic number in response header"),
            ));
        }
        let header = PacketHeader {
            magic: magic,
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

pub fn parse_header_only_response<R: io::Read>(reader: R) -> Result<(), MemcacheError> {
    let header = PacketHeader::read(reader)?;
    if header.vbucket_id_or_status != ResponseStatus::NoError as u16 {
        return Err(MemcacheError::from(header.vbucket_id_or_status));
    }
    return Ok(())
}

pub fn parse_version_response<R: io::Read>(mut reader: R) -> Result<String, MemcacheError> {
    let header = PacketHeader::read(&mut reader)?;
    if header.vbucket_id_or_status != ResponseStatus::NoError as u16 {
        return Err(MemcacheError::from(header.vbucket_id_or_status));
    }
    let mut buffer = vec![0; header.total_body_length as usize];
    reader.read_exact(buffer.as_mut_slice())?;
    return Ok(String::from_utf8(buffer)?);
}

pub fn parse_get_response<R: io::Read, V: FromMemcacheValue>(mut reader: R) -> Result<Option<V>, MemcacheError> {
    let header = PacketHeader::read(&mut reader)?;
    if header.vbucket_id_or_status == ResponseStatus::KeyNotFound as u16 {
        return Ok(None);
    } else if header.vbucket_id_or_status != ResponseStatus::NoError as u16 {
        return Err(MemcacheError::from(header.vbucket_id_or_status));
    }
    let flags = reader.read_u32::<BigEndian>()?;
    let value_length = header.total_body_length - 4; // 32bit for extras
    let mut buffer = vec![0; value_length as usize];
    reader.read_exact(buffer.as_mut_slice())?;
    return Ok(Some(FromMemcacheValue::from_memcache_value(buffer, flags)?));
}

pub fn parse_delete_response<R: io::Read>(reader: R) -> Result<bool, MemcacheError> {
    let header = PacketHeader::read(reader)?;
    if header.vbucket_id_or_status == ResponseStatus::KeyNotFound as u16 {
        return Ok(false);
    } else if header.vbucket_id_or_status != ResponseStatus::NoError as u16 {
        return Err(MemcacheError::from(header.vbucket_id_or_status));
    }
    return Ok(true);
}
