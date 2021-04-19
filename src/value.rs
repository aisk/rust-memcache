use crate::error::MemcacheError;
use std::io;
use std::io::Write;
use std::str;
use std::str::FromStr;

pub enum Flags {
    Bytes = 0,
}

/// determine how the value is serialize to memcache
pub trait ToMemcacheValue<W: Write> {
    fn get_flags(&self) -> u32;
    fn get_length(&self) -> usize;
    fn write_to(&self, stream: &mut W) -> io::Result<()>;
}

impl<'a, W: Write> ToMemcacheValue<W> for &'a [u8] {
    fn get_flags(&self) -> u32 {
        return Flags::Bytes as u32;
    }

    fn get_length(&self) -> usize {
        return self.len();
    }

    fn write_to(&self, stream: &mut W) -> io::Result<()> {
        match stream.write_all(self) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl<'a, W: Write> ToMemcacheValue<W> for &'a String {
    fn get_flags(&self) -> u32 {
        ToMemcacheValue::<W>::get_flags(*self)
    }

    fn get_length(&self) -> usize {
        ToMemcacheValue::<W>::get_length(*self)
    }

    fn write_to(&self, stream: &mut W) -> io::Result<()> {
        ToMemcacheValue::<W>::write_to(*self, stream)
    }
}

impl<W: Write> ToMemcacheValue<W> for String {
    fn get_flags(&self) -> u32 {
        return Flags::Bytes as u32;
    }

    fn get_length(&self) -> usize {
        return self.as_bytes().len();
    }

    fn write_to(&self, stream: &mut W) -> io::Result<()> {
        match stream.write_all(self.as_bytes()) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl<'a, W: Write> ToMemcacheValue<W> for &'a str {
    fn get_flags(&self) -> u32 {
        return Flags::Bytes as u32;
    }

    fn get_length(&self) -> usize {
        return self.as_bytes().len();
    }

    fn write_to(&self, stream: &mut W) -> io::Result<()> {
        match stream.write_all(self.as_bytes()) {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

macro_rules! impl_to_memcache_value_for_number {
    ($ty:ident) => {
        impl<W: Write> ToMemcacheValue<W> for $ty {
            fn get_flags(&self) -> u32 {
                return Flags::Bytes as u32;
            }

            fn get_length(&self) -> usize {
                return self.to_string().as_bytes().len();
            }

            fn write_to(&self, stream: &mut W) -> io::Result<()> {
                match stream.write_all(self.to_string().as_bytes()) {
                    Ok(_) => Ok(()),
                    Err(e) => Err(e),
                }
            }
        }
    };
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
    fn from_memcache_value(_: Vec<u8>, _: u32) -> MemcacheValue<Self>;
}

pub trait FromMemcacheValueExt: Sized {
    fn from_memcache_value(value: Vec<u8>, flags: u32, cas: Option<u64>) -> MemcacheValue<Self>;
}

impl<V: FromMemcacheValue> FromMemcacheValueExt for V {
    fn from_memcache_value(value: Vec<u8>, flags: u32, _cas: Option<u64>) -> MemcacheValue<Self> {
        FromMemcacheValue::from_memcache_value(value, flags)
    }
}

impl FromMemcacheValueExt for (Vec<u8>, u32, Option<u64>) {
    fn from_memcache_value(value: Vec<u8>, flags: u32, cas: Option<u64>) -> MemcacheValue<Self> {
        return Ok((value, flags, cas));
    }
}

impl FromMemcacheValue for (Vec<u8>, u32) {
    fn from_memcache_value(value: Vec<u8>, flags: u32) -> MemcacheValue<Self> {
        return Ok((value, flags));
    }
}

impl FromMemcacheValue for Vec<u8> {
    fn from_memcache_value(value: Vec<u8>, _: u32) -> MemcacheValue<Self> {
        return Ok(value);
    }
}

impl FromMemcacheValue for String {
    fn from_memcache_value(value: Vec<u8>, _: u32) -> MemcacheValue<Self> {
        return Ok(String::from_utf8(value)?);
    }
}

macro_rules! impl_from_memcache_value_for_number {
    ($ty:ident) => {
        impl FromMemcacheValue for $ty {
            fn from_memcache_value(value: Vec<u8>, _: u32) -> MemcacheValue<Self> {
                let s: String = FromMemcacheValue::from_memcache_value(value, 0)?;
                Ok(Self::from_str(s.as_str())?)
            }
        }
    };
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
