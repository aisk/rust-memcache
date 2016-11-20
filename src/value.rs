pub trait ToMemcacheValue {
    fn get_flags(&self) -> u16;
    fn get_bytes(&self) -> &[u8];
}

pub struct Value<'a> {
    pub bytes: &'a [u8],
    pub flags: u16,
}

impl<'a> ToMemcacheValue for &'a Value<'a> {
    fn get_flags(&self) -> u16 {
        return self.flags;
    }

    fn get_bytes(&self) -> &[u8] {
        return self.bytes;
    }
}
