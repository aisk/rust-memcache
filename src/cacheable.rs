pub trait Cacheable {
    fn to_cache(&self) -> (&[u8], u32);
}

impl <'a> Cacheable for &'a[u8] {
    fn to_cache(&self) -> (&[u8], u32) {
        return (self, 0);
    }
}

impl Cacheable for String {
    fn to_cache(&self) -> (&[u8], u32) {
        return (self.as_bytes(), 1);
    }
}
