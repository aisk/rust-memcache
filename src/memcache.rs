extern crate libc;

use std::convert::From;
use std::ffi::{
    CStr,
    CString,
};
use ffi::{
    memcached,
    memcached_add,
    memcached_exist,
    memcached_flush,
    memcached_free,
    memcached_get,
    memcached_increment,
    memcached_last_error,
    memcached_replace,
    memcached_return_t,
    memcached_set,
    memcached_st,
};
use error::{
    LibMemcachedError,
    MemcacheResult,
};
use connectable::Connectable;

enum StoreCommand {
    ADD,
    REPLACE,
    SET,
}

//#[derive(Debug)]
pub struct Memcache {
    c_st: *const memcached_st,
}

impl Drop for Memcache {
    fn drop(&mut self) {
        unsafe {
            memcached_free(self.c_st);
        }
    }
}

impl Memcache {
    pub fn connect(connectable: &Connectable) -> MemcacheResult<Memcache> {
        let s = connectable.get_connection_str();
        let cstring = CString::new(s).unwrap();
        let s_len = cstring.to_bytes().len();
        unsafe {
            let c_st = memcached(cstring.as_ptr(), s_len as u64);
            if c_st.is_null() {
                let error_code = memcached_last_error(c_st);
                return Err(From::from(LibMemcachedError::new(error_code)));
            }
            return Ok(Memcache{ c_st: c_st });
        }
    }

    /// Flush all the data on memcached after `expiration`.
    ///
    /// If `expiration` is 0, flush data immediatly.
    ///
    /// # Examples
    ///
    /// ```
    /// let mc = memcache::connect(&("localhost", 2333)).unwrap();
    /// mc.flush(0);
    /// ```
    pub fn flush(&self, expiration: libc::time_t) -> MemcacheResult<()> {
        let r = unsafe{ memcached_flush(self.c_st, expiration) };
        match r {
            memcached_return_t::MEMCACHED_SUCCESS => {
                return Ok(());
            }
            _ => {
                return Err(From::from(LibMemcachedError::new(r)));
            }
        }
    }

    /// Determin weather a key is exist on memcached.
    pub fn exist(&self, key: &str) -> MemcacheResult<bool> {
        let key = CString::new(key).unwrap();
        let key_length = key.as_bytes().len();
        let ret = unsafe{
            memcached_exist(self.c_st, key.as_ptr(), key_length as u64)
        };
        match ret {
            memcached_return_t::MEMCACHED_SUCCESS => Ok(true),
            memcached_return_t::MEMCACHED_NOTFOUND => Ok(false),
            _ => Err(From::from(LibMemcachedError::new(ret))),
        }
    }

    fn store_raw(&self, command: StoreCommand, key: &str, value: &[u8], expiration: libc::time_t, flags: u32) -> MemcacheResult<()> {
        // TODO: raise if key containes NULL
        let key = CString::new(key).unwrap();
        let key_length = key.as_bytes().len();
        let value_length = value.len();
        let value = unsafe { CString::from_vec_unchecked(value.to_vec()) };

        let store_func = match command {
            StoreCommand::SET => memcached_set,
            StoreCommand::ADD => memcached_add,
            StoreCommand::REPLACE => memcached_replace,
        };

        let r = unsafe {
            store_func(self.c_st, key.as_ptr(), key_length as u64, value.as_ptr(), value_length as u64, expiration, flags)
        };
        match r {
            memcached_return_t::MEMCACHED_SUCCESS => {
                return Ok(());
            }
            _ => {
                return Err(From::from(LibMemcachedError::new(r)));
            }
        }
    }

    /// Set bytes data to memcached.
    pub fn set_raw(&self, key: &str, value: &[u8], expiration: libc::time_t, flags: u32) -> MemcacheResult<()> {
        return self.store_raw(StoreCommand::SET, key, value, expiration, flags);
    }

    /// Set bytes data to memcached only if the key is **not** existed.
    pub fn add_raw(&self, key: &str, value: &[u8], expiration: libc::time_t, flags: u32) -> MemcacheResult<()> {
        return self.store_raw(StoreCommand::ADD, key, value, expiration, flags);
    }

    /// Set bytes data to memcached only if the key is existed.
    pub fn replace_raw(&self, key: &str, value: &[u8], expiration: libc::time_t, flags: u32) -> MemcacheResult<()> {
        return self.store_raw(StoreCommand::REPLACE, key, value, expiration, flags);
    }

    /// Get bytes data from memcached.
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
            memcached_get(self.c_st, key.as_ptr(), key_length as u64, value_length_ptr, flags_ptr, ret_ptr)
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
            _ => Err(From::from(LibMemcachedError::new(ret)))
        }
    }

    pub fn increment(&self, key: &str, offset: u32) -> MemcacheResult<u64> {
        let key = CString::new(key).unwrap();
        let key_length = key.as_bytes().len();

        let mut value: libc::uint64_t = 0;
        let value_ptr: *mut libc::uint64_t = &mut value;

        let ret = unsafe {
            memcached_increment(self.c_st, key.as_ptr(), key_length as u64, offset, value_ptr)
        };

        match ret {
            memcached_return_t::MEMCACHED_SUCCESS => {
                return Ok((value));
            }
            _ => Err(From::from(LibMemcachedError::new(ret)))
        }
    }
}
