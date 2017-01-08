use std::str::FromStr;
use std::io;
use std::io::Write;
use std::net;
use error::MemcacheError;

pub enum Flags {
    Bytes = 0,
}

/// determine how the value is serialize to memcache
pub trait ToMemcacheValue {
    fn get_flags(&self) -> u16;
    fn get_exptime(&self) -> u32;
    fn get_length(&self) -> usize;
    fn write_to(&self, stream: &net::TcpStream) -> io::Result<()>;
}

#[doc(hidden)]
pub struct Raw<'a> {
    pub bytes: &'a [u8],
    pub flags: u16,
}

impl<'a> ToMemcacheValue for (&'a [u8], u16) {
    fn get_flags(&self) -> u16 {
        return self.1;
    }

    fn get_exptime(&self) -> u32 {
        return 0;
    }

    fn get_length(&self) -> usize {
        return self.0.len();
    }

    fn write_to(&self, mut stream: &net::TcpStream) -> io::Result<()> {
        match stream.write(self.0) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl<'a> ToMemcacheValue for &'a Raw<'a> {
    fn get_flags(&self) -> u16 {
        return self.flags;
    }

    fn get_exptime(&self) -> u32 {
        return 0;
    }

    fn get_length(&self) -> usize {
        return self.bytes.len();
    }

    fn write_to(&self, mut stream: &net::TcpStream) -> io::Result<()> {
        match stream.write(self.bytes) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl<'a> ToMemcacheValue for &'a [u8] {
    fn get_flags(&self) -> u16 {
        return Flags::Bytes as u16;
    }

    fn get_exptime(&self) -> u32 {
        return 0;
    }

    fn get_length(&self) -> usize {
        return self.len();
    }

    fn write_to(&self, mut stream: &net::TcpStream) -> io::Result<()> {
        match stream.write(self) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl ToMemcacheValue for String {
    fn get_flags(&self) -> u16 {
        return Flags::Bytes as u16;
    }

    fn get_exptime(&self) -> u32 {
        return 0;
    }

    fn get_length(&self) -> usize {
        return self.as_bytes().len();
    }

    fn write_to(&self, mut stream: &net::TcpStream) -> io::Result<()> {
        match stream.write(self.as_bytes()) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl<'a> ToMemcacheValue for &'a str {
    fn get_flags(&self) -> u16 {
        return Flags::Bytes as u16;
    }

    fn get_exptime(&self) -> u32 {
        return 0;
    }

    fn get_length(&self) -> usize {
        return self.as_bytes().len();
    }

    fn write_to(&self, mut stream: &net::TcpStream) -> io::Result<()> {
        match stream.write(self.as_bytes()) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

macro_rules! impl_to_memcache_value_for_number{
    ($ty:ident) => {
        impl ToMemcacheValue for $ty {
            fn get_flags(&self) -> u16 {
                return Flags::Bytes as u16;
            }

    fn get_exptime(&self) -> u32 {
        return 0;
    }

            fn get_length(&self) -> usize {
                return self.to_string().as_bytes().len();
            }

            fn write_to(&self, mut stream: &net::TcpStream) -> io::Result<()> {
                match stream.write(self.to_string().as_bytes()) {
                    Ok(_) => Ok(()),
                    Err(e) => Err(e),
                }
            }
        }
    }
}

impl_to_memcache_value_for_number!(bool);
impl_to_memcache_value_for_number!(u8);
impl_to_memcache_value_for_number!(u16);
impl_to_memcache_value_for_number!(u32);
impl_to_memcache_value_for_number!(u64);
impl_to_memcache_value_for_number!(i8);
impl_to_memcache_value_for_number!(i16);
impl_to_memcache_value_for_number!(i32);
impl_to_memcache_value_for_number!(i64);
impl_to_memcache_value_for_number!(f32);
impl_to_memcache_value_for_number!(f64);

type MemcacheValue<T> = Result<T, MemcacheError>;

/// determine how the value is unserialize to memcache
pub trait FromMemcacheValue: Sized {
    fn from_memcache_value(Vec<u8>, u16) -> MemcacheValue<Self>;
}

impl FromMemcacheValue for (Vec<u8>, u16) {
    fn from_memcache_value(value: Vec<u8>, flags: u16) -> MemcacheValue<Self> {
        return Ok((value, flags));
    }
}

impl FromMemcacheValue for Vec<u8> {
    fn from_memcache_value(value: Vec<u8>, _: u16) -> MemcacheValue<Self> {
        return Ok(value);
    }
}

impl FromMemcacheValue for String {
    fn from_memcache_value(value: Vec<u8>, _: u16) -> MemcacheValue<Self> {
        return Ok(String::from_utf8(value)?);
    }
}

macro_rules! impl_from_memcache_value_for_number{
    ($ty:ident) => {
        impl FromMemcacheValue for $ty {
            fn from_memcache_value(value: Vec<u8>, _: u16) -> MemcacheValue<Self> {
                let s: String = String::from_memcache_value(value, 0)?;
                match Self::from_str(s.as_str()) {
                    Ok(v) => return Ok(v),
                    Err(_) => Err(MemcacheError::Error),
                }
            }
        }
    }
}

impl_from_memcache_value_for_number!(bool);
impl_from_memcache_value_for_number!(u8);
impl_from_memcache_value_for_number!(u16);
impl_from_memcache_value_for_number!(u32);
impl_from_memcache_value_for_number!(u64);
impl_from_memcache_value_for_number!(i8);
impl_from_memcache_value_for_number!(i16);
impl_from_memcache_value_for_number!(i32);
impl_from_memcache_value_for_number!(i64);
impl_from_memcache_value_for_number!(f32);
impl_from_memcache_value_for_number!(f64);
