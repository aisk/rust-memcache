extern crate memcache;

use std::collections::HashMap;
use std::{thread, time};

#[test]
fn test_ascii() {
    let client = memcache::Client::connect("memcache://localhost:12345?protocol=ascii").unwrap();

    client.flush_with_delay(1).unwrap();
    thread::sleep(time::Duration::from_secs(1));
    client.flush().unwrap();

    client.set("ascii_foo", "bar", 0).unwrap();
    let value: Option<String> = client.get("ascii_foo").unwrap();
    assert_eq!(value, Some("bar".into()));

    client.set("ascii_baz", "qux", 0).unwrap();
    let values: HashMap<String, (Vec<u8>, u32)> = client.gets(&["ascii_foo", "ascii_baz", "not_exists_key"]).unwrap();
    assert_eq!(values.len(), 2);
    let ascii_foo_value = values.get("ascii_foo").unwrap();
    let ascii_baz_value = values.get("ascii_baz").unwrap();
    assert_eq!(String::from_utf8(ascii_foo_value.0.clone()).unwrap(), "bar".to_string());
    assert_eq!(String::from_utf8(ascii_baz_value.0.clone()).unwrap(), "qux".to_string());

    client.touch("ascii_foo", 1000).unwrap();

    let value: Option<String> = client.get("not_exists_key").unwrap();
    assert_eq!(value, None);

    client.set("ascii_pend", "y", 0).unwrap();
    client.append("ascii_pend", "z").unwrap();
    let value: Option<String> = client.get("ascii_pend").unwrap();
    assert_eq!(value, Some("yz".into()));
    client.prepend("ascii_pend", "x").unwrap();
    let value: Option<String> = client.get("ascii_pend").unwrap();
    assert_eq!(value, Some("xyz".into()));

    client.delete("ascii_pend").unwrap();
    let value: Option<String> = client.get("ascii_pend").unwrap();
    assert_eq!(value, None);

    assert!(client.increment("ascii_counter", 1).is_err());
    client.set("ascii_counter", 3, 0).unwrap();
    assert_eq!(client.increment("ascii_counter", 100).unwrap(), 103);
    assert_eq!(client.decrement("ascii_counter", 3).unwrap(), 100);

    client.stats().unwrap();
}

#[test]
fn test_get_with_flags() {
    use memcache::ToMemcacheValue;
    use std::io::Write;

    let client = memcache::Client::connect("memcache://localhost:12345?protocol=ascii").unwrap();
    client.flush().unwrap();

    // Create a custom value with specific flags for testing
    struct TestValue {
        data: Vec<u8>,
        flags: u32,
    }

    impl<W: Write> ToMemcacheValue<W> for TestValue {
        fn get_flags(&self) -> u32 {
            self.flags
        }

        fn get_length(&self) -> usize {
            self.data.len()
        }

        fn write_to(&self, stream: &mut W) -> std::io::Result<()> {
            stream.write_all(&self.data)
        }
    }

    // Test with custom flags
    let test_value = TestValue {
        data: b"test_value".to_vec(),
        flags: 114514,
    };

    client.set("test_key", test_value, 0).unwrap();

    // Test our new FromMemcacheValue implementation
    let value: Option<(String, u32)> = client.get("test_key").unwrap();
    assert_eq!(value, Some(("test_value".to_string(), 114514)));

    client.flush().unwrap();
}
