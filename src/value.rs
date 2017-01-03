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

impl FromMemcacheValue for Vec<u8> {
    fn from_memcache_value(value: Vec<u8>, _: u16) -> MemcacheValue<Self> {
        return Ok(value);
    }
}
