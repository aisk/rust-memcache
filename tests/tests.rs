extern crate memcache;
extern crate rand;

use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use std::iter;
use std::thread;
use std::thread::JoinHandle;
use std::time;

fn gen_random_key() -> String {
    let bs = iter::repeat(())
        .map(|()| thread_rng().sample(Alphanumeric))
        .take(10)
        .collect::<Vec<u8>>();
    return String::from_utf8(bs).unwrap();
}

#[test]
fn test() {
    let mut urls = vec![
        "memcache://localhost:12346?tcp_nodelay=true",
        "memcache://localhost:12347?timeout=10",
        "memcache://localhost:12348?protocol=ascii",
        "memcache://localhost:12349?",
        "memcache+tls://localhost:12350?verify_mode=none",
    ];
    if cfg!(unix) {
        urls.push("memcache:///tmp/memcached2.sock");
    }
    let client = memcache::Client::connect(urls).unwrap();

    client.version().unwrap();

    client.set("foo", "bar", 0).unwrap();
    client.flush().unwrap();
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, None);

    client.set("foo", "bar", 0).unwrap();
    client.flush_with_delay(3).unwrap();
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, Some(String::from("bar")));
    thread::sleep(time::Duration::from_secs(4));
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, None);

    let mut keys: Vec<String> = Vec::new();
    for _ in 0..1000 {
        let key = gen_random_key();
        keys.push(key.clone());
        client.set(key.as_str(), "xxx", 0).unwrap();
    }

    for key in keys {
        let value: String = client.get(key.as_str()).unwrap().unwrap();
        assert_eq!(value, "xxx");
    }
}

#[test]
fn issue74() {
    use memcache::{Client, CommandError, MemcacheError};
    let client = Client::connect("memcache://localhost:12346?tcp_nodelay=true").unwrap();
    client.delete("issue74").unwrap();
    client.add("issue74", 1, 0).unwrap();

    match client.add("issue74", 1, 0) {
        Ok(_) => panic!("Should got an error!"),
        Err(e) => match e {
            MemcacheError::CommandError(e) => assert!(e == CommandError::KeyExists),
            _ => panic!("Unexpected error!"),
        },
    }

    match client.add("issue74", 1, 0) {
        Ok(_) => panic!("Should got an error!"),
        Err(e) => match e {
            MemcacheError::CommandError(e) => assert!(e == CommandError::KeyExists),
            _ => panic!("Unexpected error!"),
        },
    }

    match client.add("issue74", 1, 0) {
        Ok(_) => panic!("Should got an error!"),
        Err(e) => match e {
            MemcacheError::CommandError(e) => assert!(e == CommandError::KeyExists),
            _ => panic!("Unexpected error!"),
        },
    }
}

#[test]
fn udp_test() {
    let urls = vec!["memcache+udp://localhost:22345"];
    let client = memcache::Client::connect(urls).unwrap();

    client.version().unwrap();

    client.set("foo", "bar", 0).unwrap();
    client.flush().unwrap();
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, None);

    client.set("foo", "bar", 0).unwrap();
    client.flush_with_delay(3).unwrap();
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, Some(String::from("bar")));
    thread::sleep(time::Duration::from_secs(4));
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, None);

    client.set("foo", "bar", 0).unwrap();
    let value = client.add("foo", "baz", 0);
    assert_eq!(value.is_err(), true);

    client.delete("foo").unwrap();
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, None);

    client.add("foo", "bar", 0).unwrap();
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, Some(String::from("bar")));

    client.replace("foo", "baz", 0).unwrap();
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, Some(String::from("baz")));

    client.append("foo", "bar").unwrap();
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, Some(String::from("bazbar")));

    client.prepend("foo", "bar").unwrap();
    let value: Option<String> = client.get("foo").unwrap();
    assert_eq!(value, Some(String::from("barbazbar")));

    client.set("fooo", 0, 0).unwrap();
    client.increment("fooo", 1).unwrap();
    let value: Option<String> = client.get("fooo").unwrap();
    assert_eq!(value, Some(String::from("1")));

    client.decrement("fooo", 1).unwrap();
    let value: Option<String> = client.get("fooo").unwrap();
    assert_eq!(value, Some(String::from("0")));

    assert_eq!(client.touch("foooo", 123).unwrap(), false);
    assert_eq!(client.touch("fooo", 12345).unwrap(), true);

    // gets is not supported for udp
    let value: Result<std::collections::HashMap<String, String>, _> = client.gets(&["foo", "fooo"]);
    assert_eq!(value.is_ok(), false);

    let mut keys: Vec<String> = Vec::new();
    for _ in 0..1000 {
        let key = gen_random_key();
        keys.push(key.clone());
        client.set(key.as_str(), "xxx", 0).unwrap();
    }

    for key in keys {
        let value: String = client.get(key.as_str()).unwrap().unwrap();

        assert_eq!(value, "xxx");
    }

    // test with multiple udp connections
    let mut handles: Vec<Option<JoinHandle<_>>> = Vec::new();
    for i in 0..10 {
        handles.push(Some(thread::spawn(move || {
            let key = format!("key{}", i);
            let value = format!("value{}", i);
            let client = memcache::Client::connect("memcache://localhost:22345?udp=true").unwrap();
            for j in 0..50 {
                let value = format!("{}{}", value, j);
                client.set(key.as_str(), &value, 0).unwrap();
                let result: Option<String> = client.get(key.as_str()).unwrap();
                assert_eq!(result.as_ref(), Some(&value));

                let result = client.add(key.as_str(), &value, 0);
                assert_eq!(result.is_err(), true);

                client.delete(key.as_str()).unwrap();
                let result: Option<String> = client.get(key.as_str()).unwrap();
                assert_eq!(result, None);

                client.add(key.as_str(), &value, 0).unwrap();
                let result: Option<String> = client.get(key.as_str()).unwrap();
                assert_eq!(result.as_ref(), Some(&value));

                client.replace(key.as_str(), &value, 0).unwrap();
                let result: Option<String> = client.get(key.as_str()).unwrap();
                assert_eq!(result.as_ref(), Some(&value));

                client.append(key.as_str(), &value).unwrap();
                let result: Option<String> = client.get(key.as_str()).unwrap();
                assert_eq!(result, Some(format!("{}{}", value, value)));

                client.prepend(key.as_str(), &value).unwrap();
                let result: Option<String> = client.get(key.as_str()).unwrap();
                assert_eq!(result, Some(format!("{}{}{}", value, value, value)));
            }
        })));
    }

    for i in 0..10 {
        handles[i].take().unwrap().join().unwrap();
    }
}

#[test]
fn test_cas() {
    use memcache::Client;
    use std::collections::HashMap;
    let clients = vec![
        Client::connect("memcache://localhost:12345").unwrap(),
        Client::connect("memcache://localhost:12345?protocol=ascii").unwrap(),
    ];
    for client in clients {
        client.flush().unwrap();

        client.set("ascii_foo", "bar", 0).unwrap();
        let value: Option<String> = client.get("ascii_foo").unwrap();
        assert_eq!(value, Some("bar".into()));

        client.set("ascii_baz", "qux", 0).unwrap();

        let values: HashMap<String, (Vec<u8>, u32, Option<u64>)> =
            client.gets(&["ascii_foo", "ascii_baz", "not_exists_key"]).unwrap();
        assert_eq!(values.len(), 2);
        let ascii_foo_value = values.get("ascii_foo").unwrap();
        let ascii_baz_value = values.get("ascii_baz").unwrap();

        assert!(ascii_foo_value.2.is_some());
        assert!(ascii_baz_value.2.is_some());
        assert_eq!(
            true,
            client.cas("ascii_foo", "bar2", 0, ascii_foo_value.2.unwrap()).unwrap()
        );
        assert_eq!(
            false,
            client.cas("ascii_foo", "bar3", 0, ascii_foo_value.2.unwrap()).unwrap()
        );

        assert_eq!(
            false,
            client
                .cas("not_exists_key", "bar", 0, ascii_foo_value.2.unwrap())
                .unwrap()
        );
        client.flush().unwrap();
    }
}
