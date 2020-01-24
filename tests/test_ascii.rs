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
