extern crate libc;
use std::str;
use std::ffi;

#[repr(C)]
struct memcached_st;

#[link(name = "memcached")]
extern {
    fn memcached(string: *const libc::c_char, string_length: libc::size_t) -> *const memcached_st;
    fn memcached_last_error_message(client: *const memcached_st) -> *const libc::c_char;
}

#[test]
fn test_memcaced() {
    unsafe {
        let s = "--SERVER=localhost";
        let string = ffi::CString::new(s).unwrap();
        let client = memcached(string.as_ptr(), 18);
        println!("{:?}", client);
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
    }
}
