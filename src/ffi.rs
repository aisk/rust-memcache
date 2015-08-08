extern crate libc;
use self::libc::{size_t, c_char, time_t, uint32_t, uint64_t};

#[repr(C)]
pub struct memcached_st;

#[repr(C)]
#[derive(Debug)]
pub enum memcached_return_t {
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
    pub fn memcached(
        string: *const c_char,
        string_length: size_t
    ) -> *const memcached_st;

    pub fn memcached_exist(
        client: *const memcached_st,
        key: *const c_char,
        key_length: size_t
    ) -> memcached_return_t;

    pub fn memcached_free(client: *const memcached_st);
    
    pub fn memcached_flush(
        client: *const memcached_st,
        expiration: time_t
    ) -> memcached_return_t;

    pub fn memcached_get(
        client: *const memcached_st,
        key: *const c_char,
        key_length: size_t,
        value_length: *mut size_t,
        flags: *mut uint32_t,
        error: *mut memcached_return_t
    ) -> *const c_char;

    pub fn memcached_last_error(
        client: *const memcached_st
    ) -> memcached_return_t;

    pub fn memcached_last_error_message(
        client: *const memcached_st
    ) -> *const c_char;

    pub fn memcached_set(
        client: *const memcached_st,
        key: *const c_char,
        key_length: size_t,
        value: *const c_char,
        value_length: size_t,
        expiration: time_t,
        flags: uint32_t
    ) -> memcached_return_t;

    pub fn memcached_add(
        client: *const memcached_st,
        key: *const c_char,
        key_length: size_t,
        value: *const c_char,
        value_length: size_t,
        expiration: time_t,
        flags: uint32_t
    ) -> memcached_return_t;

    pub fn memcached_replace(
        client: *const memcached_st,
        key: *const c_char,
        key_length: size_t,
        value: *const c_char,
        value_length: size_t,
        expiration: time_t,
        flags: uint32_t
    ) -> memcached_return_t;

    pub fn memcached_increment(
        client: *const memcached_st,
        key: *const c_char,
        key_length: size_t,
        offset: uint32_t,
        value: *const uint64_t
    ) -> memcached_return_t;

    pub fn memcached_increment_with_initial(
        client: *const memcached_st,
        key: *const c_char,
        key_length: size_t,
        offset: uint64_t,
        initial: uint64_t,
        expiration: time_t,
        value: *const uint64_t
    ) -> memcached_return_t;

    pub fn memcached_strerror(
        client: *const memcached_st,
        rc: memcached_return_t
    ) -> *const c_char;
}

#[test]
fn test_memcaced() {
    use std::ffi;
    unsafe {
        let s = "--SERVER=localhost:2333";
        let string = ffi::CString::new(s).unwrap();
        let client = memcached(string.as_ptr(), 23);
        assert!(!client.is_null());
    }
}

#[test]
fn test_memcached_last_error_message() {
    use std::ffi;
    unsafe {
        let string = ffi::CString::new("foo").unwrap();
        let client = memcached(string.as_ptr(), 3);
        let error = memcached_last_error_message(client);
        // let slice = ffi::CStr::from_ptr(error);
        // println!("string returned: {}", str::from_utf8(slice.to_bytes()).unwrap());
        assert!(!error.is_null());
    }
}

#[test]
fn test_memcached_operations() {
    use std::ffi;
    use std::str;
    unsafe{
        // client
        let s = "--SERVER=localhost:2333";
        let string = ffi::CString::new(s).unwrap();
        let client = memcached(string.as_ptr(), 23);
        assert!(!client.is_null());

        // flush
        let r = memcached_flush(client, 0);
        match r {
            memcached_return_t::MEMCACHED_SUCCESS => {}
            _ => panic!()
        }

        // set foo bar
        let key = ffi::CString::new("foo").unwrap();
        let key_length = 3;
        let value = ffi::CString::new("bar").unwrap();
        let value_length = 3;
        let r = memcached_set(client, key.as_ptr(), key_length, value.as_ptr(), value_length, 0, 0);

        match r {
            memcached_return_t::MEMCACHED_SUCCESS => {}
            _ => panic!()
        }

        // get foo == bar
        let mut value_length: size_t = 0;
        let value_length_ptr: *mut size_t = &mut value_length;

        let mut flags: uint32_t = 0;
        let flags_ptr: *mut uint32_t = &mut flags;

        let mut error: memcached_return_t = memcached_return_t::MEMCACHED_FAILURE;
        let error_ptr: *mut memcached_return_t = &mut error;

        let r = memcached_get(client, key.as_ptr(), key_length, value_length_ptr, flags_ptr, error_ptr);

        // println!("value: {:?}, error: {:?}, value_length: {:?}", r, error_ptr, value_length_ptr);
        assert!(value_length == 3);
        match error {
            memcached_return_t::MEMCACHED_SUCCESS => {}
            _ => panic!()
        }
        let slice = ffi::CStr::from_ptr(r);
        assert!(str::from_utf8(slice.to_bytes()).unwrap() == "bar");
    }
}
