extern crate libc;
use std::str;
use std::ffi;
use self::libc::{size_t, c_char, time_t, uint32_t};

#[repr(C)]
struct memcached_st;

#[repr(C)]
#[derive(Debug)]
enum memcached_return_t {
    MEMCACHED_SUCCESS,
    MEMCACHED_FAILURE,
    MEMCACHED_HOST_LOOKUP_FAILURE, // getaddrinfo() and getnameinfo() only
    MEMCACHED_CONNECTION_FAILURE,
    MEMCACHED_CONNECTION_BIND_FAILURE,  // DEPRECATED, see MEMCACHED_HOST_LOOKUP_FAILURE
    MEMCACHED_WRITE_FAILURE,
    MEMCACHED_READ_FAILURE,
    MEMCACHED_UNKNOWN_READ_FAILURE,
    MEMCACHED_PROTOCOL_ERROR,
    MEMCACHED_CLIENT_ERROR,
    MEMCACHED_SERVER_ERROR, // Server returns "SERVER_ERROR"
    MEMCACHED_ERROR, // Server returns "ERROR"
    MEMCACHED_DATA_EXISTS,
    MEMCACHED_DATA_DOES_NOT_EXIST,
    MEMCACHED_NOTSTORED,
    MEMCACHED_STORED,
    MEMCACHED_NOTFOUND,
    MEMCACHED_MEMORY_ALLOCATION_FAILURE,
    MEMCACHED_PARTIAL_READ,
    MEMCACHED_SOME_ERRORS,
    MEMCACHED_NO_SERVERS,
    MEMCACHED_END,
    MEMCACHED_DELETED,
    MEMCACHED_VALUE,
    MEMCACHED_STAT,
    MEMCACHED_ITEM,
    MEMCACHED_ERRNO,
    MEMCACHED_FAIL_UNIX_SOCKET, // DEPRECATED
    MEMCACHED_NOT_SUPPORTED,
    MEMCACHED_NO_KEY_PROVIDED, /* Deprecated. Use MEMCACHED_BAD_KEY_PROVIDED! */
    MEMCACHED_FETCH_NOTFINISHED,
    MEMCACHED_TIMEOUT,
    MEMCACHED_BUFFERED,
    MEMCACHED_BAD_KEY_PROVIDED,
    MEMCACHED_INVALID_HOST_PROTOCOL,
    MEMCACHED_SERVER_MARKED_DEAD,
    MEMCACHED_UNKNOWN_STAT_KEY,
    MEMCACHED_E2BIG,
    MEMCACHED_INVALID_ARGUMENTS,
    MEMCACHED_KEY_TOO_BIG,
    MEMCACHED_AUTH_PROBLEM,
    MEMCACHED_AUTH_FAILURE,
    MEMCACHED_AUTH_CONTINUE,
    MEMCACHED_PARSE_ERROR,
    MEMCACHED_PARSE_USER_ERROR,
    MEMCACHED_DEPRECATED,
    MEMCACHED_IN_PROGRESS,
    MEMCACHED_SERVER_TEMPORARILY_DISABLED,
    MEMCACHED_SERVER_MEMORY_ALLOCATION_FAILURE,
    MEMCACHED_MAXIMUM_RETURN, /* Always add new error code before */
    // MEMCACHED_CONNECTION_SOCKET_CREATE_FAILURE= MEMCACHED_ERROR
}

#[link(name = "memcached")]
extern {
    fn memcached(string: *const c_char, string_length: size_t) -> *const memcached_st;
    fn memcached_last_error_message(client: *const memcached_st) -> *const c_char;
    fn memcached_strerror(client: *const memcached_st, rc: memcached_return_t) -> *const c_char;
    fn memcached_set(client: *const memcached_st, key: *const c_char, key_length: size_t, value: *const c_char, value_length: size_t, expiration: time_t, flag: uint32_t) -> memcached_return_t;
    fn memcached_flush(client: *const memcached_st, expiration: time_t) -> memcached_return_t;

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

        match r {
            memcached_return_t::MEMCACHED_SUCCESS => {}
            _ => assert!(false)
        }
    }
}

#[test]
fn test_memcached_flush() {
    unsafe{
        let s = "--SERVER=localhost";
        let string = ffi::CString::new(s).unwrap();
        let client = memcached(string.as_ptr(), 18);
        assert!(!client.is_null());

        let r = memcached_flush(client, 0);
        match r {
            memcached_return_t::MEMCACHED_SUCCESS => {}
            _ => assert!(false)
        }
    }
}
