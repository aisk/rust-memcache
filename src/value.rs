use std::str::FromStr;
use error::MemcacheError;

pub enum Flags {
    Bytes = 0,
    JSON = 1,
}

pub trait ToMemcacheValue {
    fn get_flags(&self) -> u16;
    fn get_bytes(&self) -> &[u8];
}

pub struct Raw<'a> {
    pub bytes: &'a [u8],
    pub flags: u16,
}

impl<'a> ToMemcacheValue for (&'a [u8], u16) {
    fn get_flags(&self) -> u16 {
        return self.1;
    }

    fn get_bytes(&self) -> &[u8] {
        return self.0;
    }
}

impl<'a> ToMemcacheValue for &'a Raw<'a> {
    fn get_flags(&self) -> u16 {
        return self.flags;
    }

    fn get_bytes(&self) -> &[u8] {
        return self.bytes;
    }
}

impl<'a> ToMemcacheValue for &'a [u8] {
    fn get_flags(&self) -> u16 {
        return Flags::Bytes as u16;
    }

    fn get_bytes(&self) -> &[u8] {
        return self;
    }
}

impl ToMemcacheValue for String {
    fn get_flags(&self) -> u16 {
        return Flags::Bytes as u16;
    }

    fn get_bytes(&self) -> &[u8] {
        return self.as_bytes();
    }
}

impl<'a> ToMemcacheValue for &'a str {
    fn get_flags(&self) -> u16 {
        return Flags::Bytes as u16;
    }

    fn get_bytes(&self) -> &[u8] {
        return self.as_bytes();
    }
}

type MemcacheValue<T> = Result<T, MemcacheError>;

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
        // TODO
        match String::from_utf8(value) {
            Ok(v) => Ok(v),
            Err(_) =>Err(MemcacheError::Error),
        }
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
