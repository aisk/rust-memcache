use std::ptr;
use std::ffi::CString;
use ffi::{
    memcached_st,
    memcached_flush,
    memcached,
};
use error::{
    MemcacheError,
    MemcacheResult,
};

#[derive(Debug)]
pub struct Client {
    c_client: *const memcached_st,
}


impl Client {
    pub fn connect(host: &str, port: u16) -> MemcacheResult<Client> {
        let mut s = "--SERVER=".to_string();
        s.push_str(host);
        s.push(':');
        s.push_str(&port.to_string());
        let cstring = CString::new(s).unwrap();
        let s_len = cstring.to_bytes().len();
        unsafe {
            let c_client = memcached(cstring.as_ptr(), s_len as u64);
            return Ok(Client{ c_client: c_client });
        }
    }
    pub fn flush(&self) {
        unsafe {
            memcached_flush(self.c_client, 0);
        }
    }
}
