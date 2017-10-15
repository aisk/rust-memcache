use std::io;
use byteorder::{WriteBytesExt, ReadBytesExt, BigEndian};
use error::MemcacheError;

#[allow(dead_code)]
pub enum Opcode {
    Get = 0x00,
    Set = 0x01,
    Add = 0x02,
    Repalce = 0x03,
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

#[allow(dead_code)]
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
    pub fn write<T: io::Write>(self, mut writer: T) -> Result<(), io::Error> {
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

    pub fn read<T: io::Read>(mut reader: T) -> Result<PacketHeader, MemcacheError> {
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
