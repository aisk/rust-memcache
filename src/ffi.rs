extern crate libc;
use std::str;
use std::ffi;
use self::libc::{size_t, c_char, time_t, uint32_t};

#[repr(C)]
struct memcached_st;

#[repr(C)]
struct memcached_return_t;

#[link(name = "memcached")]
extern {
    fn memcached(string: *const c_char, string_length: size_t) -> *const memcached_st;
    fn memcached_last_error_message(client: *const memcached_st) -> *const c_char;
    fn memcached_set(client: *const memcached_st, key: *const c_char, key_length: size_t, value: *const c_char, value_length: size_t, expire: time_t, flag: uint32_t);
}

#[test]
fn test_memcaced() {
    unsafe {
        let s = "--SERVER=localhost";
        let string = ffi::CString::new(s).unwrap();
        let client = memcached(string.as_ptr(), 18);
        println!("{:?}", client);
        assert!(!client.is_null());
    }
}

#[test]
fn test_memcached_last_error_message() {
    unsafe {
        let string = ffi::CString::new("foo").unwrap();
        let client = memcached(string.as_ptr(), 3);
        let error = memcached_last_error_message(client);
        let slice = ffi::CStr::from_ptr(error);
        println!("string returned: {}", str::from_utf8(slice.to_bytes()).unwrap());
        assert!(!error.is_null());
    }
}

#[test]
fn test_memcached_set() {
    unsafe{
        let s = "--SERVER=localhost";
        let string = ffi::CString::new(s).unwrap();
        let client = memcached(string.as_ptr(), 18);
        assert!(!client.is_null());

        let key = ffi::CString::new("foo").unwrap().as_ptr();
        let key_length = 3;
        let value = ffi::CString::new("bar").unwrap().as_ptr();
        let value_length = 3;
        let r = memcached_set(client, key, key_length, value, value_length, 0, 0);
        println!("{:?}", r);

        let error = memcached_last_error_message(client);
        let slice = ffi::CStr::from_ptr(error);
        println!("string returned: {}", str::from_utf8(slice.to_bytes()).unwrap());
    }
}
