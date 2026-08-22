//! Integration tests of the blocking high-level client against a live
//! memcached on localhost:12345 (and :12346 for multi-server).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use memcache::exp::{Error, ErrorKind, GetStatus, Memcache, Ttl};
use rand::distr::{Alphanumeric, SampleString};
use rand::rng;

const SERVER: &str = "localhost:12345";

fn random_key() -> String {
    Alphanumeric.sample_string(&mut rng(), 10)
}

fn cache() -> Memcache {
    Memcache::connect([SERVER]).unwrap()
}

#[test]
fn object_cache_roundtrip() {
    let cache = cache();
    let key = random_key();

    assert_eq!(cache.get::<String>(&key).unwrap(), None);
    cache.set(&key, "hello", Ttl::secs(60)).unwrap();
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("hello"));
    // The protocol layer sees the bytes and the string flag.
    let raw = cache.meta().get(&key).send().unwrap();
    assert_eq!(raw.client_flags, Some(memcache::exp::FLAG_STR));

    cache.set(&key, 42u64, Ttl::secs(60)).unwrap();
    assert_eq!(cache.get::<u64>(&key).unwrap(), Some(42));
    assert!(matches!(cache.get::<u8>(&key), Ok(Some(42))));
    cache.set(&key, "text", Ttl::secs(60)).unwrap();
    assert!(matches!(cache.get::<u64>(&key), Err(Error::Decode(_))));

    // Owned and borrowed values both encode.
    let owned = String::from("owned");
    cache.set(&key, &owned, Ttl::secs(60)).unwrap();
    cache.set(&key, owned, Duration::from_secs(60)).unwrap();
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("owned"));

    cache.delete(&key).unwrap();
    cache.delete(&key).unwrap();
    assert_eq!(cache.get::<Vec<u8>>(&key).unwrap(), None);
}

#[test]
fn zero_byte_rule() {
    let cache = cache();
    let key = random_key();

    assert!(matches!(cache.set(&key, "", Ttl::secs(60)), Err(Error::EmptyValue)));
    assert!(matches!(
        cache.add(&key, Vec::<u8>::new(), Ttl::secs(60)),
        Err(Error::EmptyValue)
    ));
    // A zero-byte item stored through the protocol layer reads as a miss.
    cache.meta().set(&key, "").send().unwrap();
    assert_eq!(cache.get::<String>(&key).unwrap(), None);
    assert_eq!(cache.take::<String>(&key).unwrap(), None);
}

#[test]
fn usage_errors() {
    let cache = cache();
    let key = random_key();
    assert!(matches!(cache.set(&key, "x", Ttl::secs(0)), Err(Error::Usage(_))));
    assert!(matches!(cache.set("", "x", Ttl::secs(60)), Err(Error::Usage(_))));
    assert!(matches!(cache.get::<String>(vec![b'x'; 251]), Err(Error::Usage(_))));
    assert!(matches!(cache.invalidate(&key, Duration::ZERO), Err(Error::Usage(_))));
    assert!(matches!(cache.append(&key, b"", Ttl::secs(60)), Err(Error::Usage(_))));
    assert!(matches!(
        cache.fetch(&key, Ttl::secs(10).refresh_ahead(Duration::from_secs(10)), || Ok::<
            _,
            Error,
        >(
            String::from("x")
        )),
        Err(Error::Usage(_))
    ));
}

#[test]
fn many_family() {
    let cache = cache();
    let keys: Vec<String> = (0..5).map(|_| random_key()).collect();

    cache
        .set_many(keys.iter().take(3).map(|k| (k.as_str(), k.as_str())), Ttl::secs(60))
        .unwrap();
    let found: HashMap<&str, String> = cache.get_many(keys.iter().map(String::as_str)).unwrap();
    assert_eq!(found.len(), 3);
    for k in keys.iter().take(3) {
        assert_eq!(found[k.as_str()], *k);
    }
    assert!(!found.contains_key(keys[3].as_str()));

    cache.delete_many(keys.iter().map(String::as_str)).unwrap();
    let found: HashMap<String, String> = cache.get_many(keys.clone()).unwrap();
    assert!(found.is_empty());

    assert!(matches!(
        cache.set_many([(keys[0].as_str(), "")], Ttl::secs(60)),
        Err(Error::EmptyValue)
    ));
}

#[test]
fn conditional_writes() {
    let cache = cache();
    let key = random_key();

    assert!(!cache.replace(&key, "x", Ttl::secs(60)).unwrap());
    assert!(cache.add(&key, "first", Ttl::secs(60)).unwrap());
    assert!(!cache.add(&key, "second", Ttl::secs(60)).unwrap());
    assert!(cache.replace(&key, "third", Ttl::secs(60)).unwrap());
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("third"));
}

#[test]
fn sessions_touch_and_inspect() {
    let cache = cache();
    let key = random_key();

    assert!(cache.inspect(&key).unwrap().is_none());
    cache.set(&key, "session", Ttl::secs(10)).unwrap();
    assert_eq!(
        cache.get_and_touch::<String>(&key, Ttl::secs(1000)).unwrap().as_deref(),
        Some("session")
    );
    let info = cache.inspect(&key).unwrap().unwrap();
    assert!(info.ttl.unwrap() > Duration::from_secs(900), "{info:?}");
    assert_eq!(info.size, 7);

    cache.touch(&key, Ttl::NEVER).unwrap();
    assert_eq!(cache.inspect(&key).unwrap().unwrap().ttl, None);
    cache.touch(random_key(), Ttl::secs(5)).unwrap();
}

#[test]
fn counters() {
    let cache = cache();
    let key = random_key();

    assert_eq!(cache.incr(&key, 5, Ttl::secs(60)).unwrap(), 5);
    assert_eq!(cache.incr(&key, 1, Ttl::secs(60)).unwrap(), 6);
    assert_eq!(cache.decr(&key, 10, Ttl::secs(60)).unwrap(), 0);
    let other = random_key();
    assert_eq!(cache.decr(&other, 3, Ttl::NEVER).unwrap(), 0);
    assert_eq!(cache.get::<u64>(&key).unwrap(), Some(0));
}

#[test]
fn byte_streams() {
    let cache = cache();
    let key = random_key();

    cache.append(&key, b"a;", Ttl::secs(60)).unwrap();
    cache.append(&key, b"b;", Ttl::secs(60)).unwrap();
    cache.prepend(&key, b"0;", Ttl::secs(60)).unwrap();
    assert_eq!(cache.take::<Vec<u8>>(&key).unwrap(), Some(b"0;a;b;".to_vec()));
    assert_eq!(cache.take::<Vec<u8>>(&key).unwrap(), None);
    assert_eq!(cache.get::<Vec<u8>>(&key).unwrap(), None);
}

#[test]
fn update_transforms() {
    let cache = cache();
    let key = random_key();

    let n: u64 = cache
        .update(&key, Ttl::secs(60), |current| current.unwrap_or(0) + 1)
        .unwrap();
    assert_eq!(n, 1);
    let n: u64 = cache
        .update(&key, Ttl::secs(60), |current| current.unwrap() + 1)
        .unwrap();
    assert_eq!(n, 2);
    assert_eq!(cache.get::<u64>(&key).unwrap(), Some(2));

    let error = cache
        .try_update::<u64, _, _>(&key, Ttl::secs(60), |_| Err(std::io::Error::other("nope")))
        .unwrap_err();
    assert!(error.callback::<std::io::Error>().is_some());
    assert_eq!(cache.get::<u64>(&key).unwrap(), Some(2));

    assert!(matches!(
        cache.update::<String, _>(&key, Ttl::secs(60), |_| String::new()),
        Err(Error::EmptyValue)
    ));

    // Concurrent updates never lose increments. The retry budget (8) is a
    // policy, so a thread that exhausts it under this artificial contention
    // simply tries again; what matters is that every applied update counts.
    let cache = Arc::new(cache);
    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let barrier = Arc::clone(&barrier);
            let key = key.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let mut applied = 0;
                while applied < 20 {
                    match cache.update::<u64, _>(&key, Ttl::secs(60), |current| current.unwrap_or(0) + 1) {
                        Ok(_) => applied += 1,
                        Err(Error::Conflict) => {}
                        Err(error) => panic!("{error}"),
                    }
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(cache.get::<u64>(&key).unwrap(), Some(82));
}

#[test]
fn update_and_take_treat_stale_as_miss_and_return_the_token() {
    let cache = cache();
    let key = random_key();

    cache.set(&key, "old", Ttl::secs(60)).unwrap();
    cache.invalidate(&key, Duration::from_secs(60)).unwrap();
    // Plain reads keep serving the stale value.
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("old"));
    assert_eq!(cache.take::<String>(&key).unwrap(), None);
    // The token was handed back: a fetch reader still wins the election.
    let winner = cache.meta().get(&key).lease_ttl(30).send().unwrap();
    assert!(winner.is_stale() && winner.won_lease(), "{winner:?}");
    // Hand it back again through update, which sees a miss and writes by CAS.
    let value: String = cache
        .update(&key, Ttl::secs(60), |current: Option<String>| {
            assert_eq!(current, None);
            String::from("new")
        })
        .unwrap();
    assert_eq!(value, "new");
    let fresh = cache.meta().get(&key).send().unwrap();
    assert!(!fresh.is_stale());
    assert_eq!(fresh.value.as_deref(), Some(&b"new"[..]));
}

#[test]
fn fetch_computes_once_and_writes_back() {
    let cache = cache();
    let key = random_key();
    let calls = AtomicUsize::new(0);
    let load = || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, Error>(String::from("computed"))
    };

    assert_eq!(cache.fetch(&key, Ttl::secs(60), load).unwrap(), "computed");
    assert_eq!(cache.fetch(&key, Ttl::secs(60), load).unwrap(), "computed");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("computed"));
    assert_eq!(cache.meta().get(&key).send().unwrap().status, GetStatus::Hit);
}

#[test]
fn fetch_loader_error_releases_the_lease() {
    let cache = cache();
    let key = random_key();

    let error = cache
        .fetch::<String, _, _>(&key, Ttl::secs(60), || Err(std::io::Error::other("db down")))
        .unwrap_err();
    assert!(error.callback::<std::io::Error>().is_some(), "{error:?}");
    // The placeholder is gone, so the next reader re-elects immediately.
    let next = cache.meta().get(&key).lease_ttl(30).send().unwrap();
    assert_eq!(next.status, GetStatus::Miss);
    assert!(next.won_lease());
}

#[test]
fn fetch_in_process_singleflight() {
    let cache = Arc::new(cache());
    let key = random_key();
    let calls = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            let barrier = Arc::clone(&barrier);
            let key = key.clone();
            std::thread::spawn(move || {
                barrier.wait();
                cache
                    .fetch(&key, Ttl::secs(60), || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(100));
                        Ok::<_, Error>(String::from("shared"))
                    })
                    .unwrap()
            })
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), "shared");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn fetch_waiters_share_the_loader_error_and_survive_a_panic() {
    let cache = Arc::new(cache());
    let key = random_key();
    let barrier = Arc::new(Barrier::new(2));
    let leader = {
        let cache = Arc::clone(&cache);
        let barrier = Arc::clone(&barrier);
        let key = key.clone();
        std::thread::spawn(move || {
            let result = cache.fetch::<String, Error, _>(&key, Ttl::secs(60), || {
                barrier.wait();
                std::thread::sleep(Duration::from_millis(100));
                panic!("loader exploded")
            });
            result.unwrap_err().to_string()
        })
    };
    barrier.wait();
    let waiter = cache.fetch(&key, Ttl::secs(60), || Ok::<_, Error>(String::from("never")));
    let error = waiter.unwrap_err();
    assert!(matches!(error, Error::Callback(_)), "{error:?}");
    assert!(leader.join().is_err());
}

#[test]
fn fetch_stale_grace_elects_one_refresher() {
    let cache = cache();
    let key = random_key();

    cache.set(&key, "old", Ttl::secs(60)).unwrap();
    cache.invalidate(&key, Duration::from_secs(60)).unwrap();
    let calls = AtomicUsize::new(0);
    let load = || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, Error>(String::from("new"))
    };
    // The blocking winner recomputes synchronously and returns the new value.
    assert_eq!(cache.fetch(&key, Ttl::secs(60), load).unwrap(), "new");
    assert_eq!(cache.fetch(&key, Ttl::secs(60), load).unwrap(), "new");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!cache.meta().get(&key).send().unwrap().is_stale());
}

#[test]
fn fetch_refresh_ahead_recomputes_inside_the_window() {
    let cache = cache();
    let key = random_key();
    let calls = AtomicUsize::new(0);
    let load = || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, Error>(String::from("v"))
    };
    let freshness = Ttl::secs(100).refresh_ahead(Duration::from_secs(30));
    cache.fetch(&key, freshness, load).unwrap();
    cache.fetch(&key, freshness, load).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Shrink the ttl into the window: the next fetch is elected and
    // recomputes, the one after it sees the renewed ttl.
    cache.touch(&key, Ttl::secs(10)).unwrap();
    cache.fetch(&key, freshness, load).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    cache.fetch(&key, freshness, load).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(cache.inspect(&key).unwrap().unwrap().ttl.unwrap() > Duration::from_secs(60));
}

#[test]
fn fetch_replaces_a_real_zero_byte_item() {
    let cache = cache();
    let key = random_key();
    cache.meta().set(&key, "").send().unwrap();
    let start = std::time::Instant::now();
    assert_eq!(
        cache
            .fetch(&key, Ttl::secs(60), || Ok::<_, Error>(String::from("filled")))
            .unwrap(),
        "filled"
    );
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "paid the cross-process backoff"
    );
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("filled"));
}

#[test]
fn fetch_waits_for_another_process_then_computes_locally() {
    let cache = cache();
    let key = random_key();
    // Simulate another process holding the lease: a lease read vivifies the
    // placeholder and nobody fills it.
    let other = cache.meta().get(&key).lease_ttl(30).send().unwrap();
    assert!(other.won_lease());
    let start = std::time::Instant::now();
    assert_eq!(
        cache
            .fetch(&key, Ttl::secs(60), || Ok::<_, Error>(String::from("local")))
            .unwrap(),
        "local"
    );
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(700) && elapsed < Duration::from_secs(3),
        "{elapsed:?}"
    );
    // No write-back without a lease: the placeholder is still there.
    assert_eq!(cache.get::<String>(&key).unwrap(), None);
}

#[test]
fn fetch_empty_loader_result() {
    let key = random_key();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let cache = Memcache::builder()
        .on_error(move |event| sink.lock().unwrap().push((event.kind, event.op)))
        .connect([SERVER])
        .unwrap();
    assert!(matches!(
        cache.fetch(&key, Ttl::secs(60), || Ok::<_, Error>(String::new())),
        Err(Error::EmptyValue)
    ));
    assert!(events.lock().unwrap().contains(&(ErrorKind::WriteBack, "fetch")));
    let next = cache.meta().get(&key).lease_ttl(30).send().unwrap();
    assert!(next.won_lease(), "lease was not released");
}

#[test]
fn degrade_folds_outages_into_misses() {
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = dead.local_addr().unwrap();
    drop(dead);
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let cache = Memcache::builder()
        .degrade(true)
        .connect_timeout(Duration::from_millis(200))
        .on_error(move |event| sink.lock().unwrap().push((event.kind, event.op)))
        .connect([addr])
        .unwrap();

    assert_eq!(cache.get::<String>("k").unwrap(), None);
    assert_eq!(cache.get_and_touch::<String>("k", Ttl::secs(5)).unwrap(), None);
    assert!(cache.get_many::<String, _>(["a", "b"]).unwrap().is_empty());
    assert!(cache.inspect("k").unwrap().is_none());
    cache.set("k", "v", Ttl::secs(5)).unwrap();
    cache.set_many([("a", "1")], Ttl::secs(5)).unwrap();
    cache.delete("k").unwrap();
    cache.delete_many(["a"]).unwrap();
    cache.invalidate("k", Duration::from_secs(5)).unwrap();
    cache.touch("k", Ttl::secs(5)).unwrap();
    cache.append("k", b"x", Ttl::secs(5)).unwrap();
    assert_eq!(
        cache
            .fetch("k", Ttl::secs(5), || Ok::<_, Error>(String::from("local")))
            .unwrap(),
        "local"
    );
    // Business-decision verbs still fail.
    assert!(cache.add("k", "v", Ttl::secs(5)).is_err());
    assert!(cache.replace("k", "v", Ttl::secs(5)).is_err());
    assert!(cache.incr("k", 1, Ttl::secs(5)).is_err());
    assert!(cache.take::<String>("k").is_err());
    assert!(cache.update::<u64, _>("k", Ttl::secs(5), |c| c.unwrap_or(0)).is_err());
    // Usage errors penetrate.
    assert!(matches!(cache.set("k", "", Ttl::secs(5)), Err(Error::EmptyValue)));
    assert!(matches!(cache.set("", "v", Ttl::secs(5)), Err(Error::Usage(_))));

    let events = events.lock().unwrap();
    assert!(events.iter().all(|(kind, _)| *kind == ErrorKind::Degraded));
    assert!(events.iter().any(|(_, op)| *op == "fetch"));
    assert!(events.len() >= 12, "{events:?}");
}

#[test]
fn without_degrade_outages_are_errors() {
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = dead.local_addr().unwrap();
    drop(dead);
    let cache = Memcache::builder()
        .connect_timeout(Duration::from_millis(200))
        .connect([addr])
        .unwrap();
    let error = cache.get::<String>("k").unwrap_err();
    assert!(error.is_retryable(), "{error:?}");
    assert!(cache.fetch("k", Ttl::secs(5), || Ok::<_, Error>(1u64)).is_err());
}

#[test]
fn multi_server_routing() {
    let cache = Memcache::connect(["localhost:12345", "localhost:12346"]).unwrap();
    let keys: Vec<String> = (0..20).map(|_| random_key()).collect();
    cache
        .set_many(keys.iter().map(|k| (k.as_str(), k.as_str())), Ttl::secs(60))
        .unwrap();
    let found: HashMap<&str, String> = cache.get_many(keys.iter().map(String::as_str)).unwrap();
    assert_eq!(found.len(), 20);
    for k in &keys {
        assert_eq!(cache.get::<String>(k).unwrap().as_deref(), Some(k.as_str()));
    }
    cache.delete_many(&keys).unwrap();
    let found: HashMap<&String, String> = cache.get_many(&keys).unwrap();
    assert!(found.is_empty());
}

#[cfg(feature = "serde_json")]
#[test]
fn json_values() {
    use memcache::exp::Json;
    #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Clone)]
    struct User {
        name: String,
        age: u32,
    }
    let cache = cache();
    let key = random_key();
    let user = User {
        name: "ann".into(),
        age: 30,
    };
    cache.set(&key, Json(&user), Ttl::secs(60)).unwrap();
    let loaded = cache.get::<Json<User>>(&key).unwrap().unwrap().into_inner();
    assert_eq!(loaded, user);
    let older: Json<User> = cache
        .update(&key, Ttl::secs(60), |current: Option<Json<User>>| {
            let mut user = current.unwrap().into_inner();
            user.age += 1;
            Json(user)
        })
        .unwrap();
    assert_eq!(older.age, 31);
}
