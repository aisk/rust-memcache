extern crate libc;

use std::ffi::CString;
use std::mem;
use std::ptr;
use std::slice;
use ffi::{
    memcached,
    memcached_flush,
    memcached_get,
    memcached_last_error,
    memcached_return_t,
    memcached_set,
    memcached_st,
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
            if c_client.is_null() {
                let error_code = memcached_last_error(c_client);
                return Err(MemcacheError::new(error_code));
            }
            return Ok(Client{ c_client: c_client });
        }
    }

    pub fn flush(&self, expiration: libc::time_t) -> MemcacheResult<()> {
        let r = unsafe{ memcached_flush(self.c_client, expiration) };
        match r {
            memcached_return_t::MEMCACHED_SUCCESS => {
                return Ok(());
            }
            _ => {
                return Err(MemcacheError::new(r));
            }
        }
    }

    pub fn set_raw(&self, key: &str, value: &[u8], expiration: libc::time_t, flags: u32) -> MemcacheResult<()> {
        // TODO: raise if key containes NULL
        let key = CString::new(key).unwrap();
        let key_length = key.as_bytes().len();
        let value_length = value.len();
        let value = unsafe { CString::from_vec_unchecked(value.to_vec()) };
        let r = unsafe {
            memcached_set(self.c_client, key.as_ptr(), key_length as u64, value.as_ptr(), value_length as u64, expiration, flags)
        };
        match r {
            memcached_return_t::MEMCACHED_SUCCESS => {
                return Ok(());
            }
            _ => {
                return Err(MemcacheError::new(r));
            }
        }
    }

    pub fn get_raw(&self, key: &str) -> MemcacheResult<(&[i8], u32)> {
        // TODO: raise if key containes NULL
        let key = CString::new(key).unwrap();
        let key_length = key.as_bytes().len();

        let mut value_length: libc::size_t = 0;
        let value_length_ptr: *mut libc::size_t = &mut value_length;

        let mut flags: libc::uint32_t = 0;
        let flags_ptr: *mut libc::uint32_t = &mut flags;

        let mut error: memcached_return_t = memcached_return_t::MEMCACHED_FAILURE;
        let error_ptr: *mut memcached_return_t = &mut error;

        let value_ptr = unsafe {
            memcached_get(self.c_client, key.as_ptr(), key_length as u64, value_length_ptr, flags_ptr, error_ptr)
        };

        // println!("value: {:?}, error: {:?}, value_length: {:?}", r, error_ptr, value_length_ptr);
        match error {
            memcached_return_t::MEMCACHED_SUCCESS => {
                let value = unsafe {
                    mem::transmute(slice::from_raw_parts(value_ptr, value_length as usize))
                };
                return Ok((value, flags));
            }
            _ => Err(MemcacheError::new(error))
        }
    }
}
