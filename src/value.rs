pub enum Flags {
    Bytes = 0,
    String = 1,
}

pub trait ToMemcacheValue {
    fn get_flags(&self) -> u16;
    fn get_bytes(&self) -> &[u8];
}

pub struct Raw<'a> {
    pub bytes: &'a [u8],
    pub flags: u16,
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
        return Flags::String as u16;
    }

    fn get_bytes(&self) -> &[u8] {
        return self.as_bytes();
    }
}

impl<'a> ToMemcacheValue for &'a str {
    fn get_flags(&self) -> u16 {
        return Flags::String as u16;
    }

    fn get_bytes(&self) -> &[u8] {
        return self.as_bytes();
    }
}
