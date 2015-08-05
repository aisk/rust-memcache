extern crate libc;

use std::ffi::{
    CStr,
    CString,
};
use ffi::{
    memcached,
    memcached_exist,
    memcached_free,
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

impl Drop for Client {
    fn drop(&mut self) {
        unsafe {
            memcached_free(self.c_client);
        }
    }
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

    pub fn exist(&self, key: &str) -> MemcacheResult<bool> {
        let key = CString::new(key).unwrap();
        let key_length = key.as_bytes().len();
        let ret = unsafe{
            memcached_exist(self.c_client, key.as_ptr(), key_length as u64)
        };
        match ret {
            memcached_return_t::MEMCACHED_SUCCESS => Ok(true),
            memcached_return_t::MEMCACHED_NOTFOUND => Ok(false),
            _ => Err(MemcacheError::new(ret)),
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

    pub fn get_raw(&self, key: &str) -> MemcacheResult<(Vec<u8>, u32)> {
        // TODO: raise if key containes NULL
        let key = CString::new(key).unwrap();
        let key_length = key.as_bytes().len();

        let mut value_length: libc::size_t = 0;
        let value_length_ptr: *mut libc::size_t = &mut value_length;

        let mut flags: libc::uint32_t = 0;
        let flags_ptr: *mut libc::uint32_t = &mut flags;

        let mut ret: memcached_return_t = memcached_return_t::MEMCACHED_FAILURE;
        let ret_ptr: *mut memcached_return_t = &mut ret;

        let raw_value: *const libc::c_char = unsafe {
            memcached_get(self.c_client, key.as_ptr(), key_length as u64, value_length_ptr, flags_ptr, ret_ptr)
        };

        // println!("value: {:?}, error: {:?}, value_length: {:?}", r, error_ptr, value_length_ptr);
        match ret {
            memcached_return_t::MEMCACHED_SUCCESS => {
                unsafe {
                    let value_c_str = CStr::from_ptr(raw_value);
                    let value = value_c_str.to_bytes().to_vec(); // TODO: here have a memory copy
                    libc::free(raw_value as *mut libc::c_void);
                    return Ok((value, flags));
                }
            }
            _ => Err(MemcacheError::new(ret))
        }
    }
}
