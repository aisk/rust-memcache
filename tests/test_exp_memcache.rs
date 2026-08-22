//! Integration tests for the exp scenario layer (`Memcache` and
//! `AsyncMemcache`) against a real memcached started by
//! `tests/setup_tests.sh`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use memcache::exp::{Error, Memcache, Ttl};
use rand::distr::{Alphanumeric, SampleString};
use rand::rng;

const SERVER: &str = "localhost:12345";

fn gen_random_key() -> String {
    Alphanumeric.sample_string(&mut rng(), 16)
}

fn connect() -> Memcache {
    Memcache::connect([SERVER]).unwrap()
}

#[test]
fn memcache_set_get_delete() {
    let cache = connect();
    let key = gen_random_key();

    assert_eq!(cache.get::<String>(&key).unwrap(), None);
    cache.set(&key, "hello", Ttl::secs(60)).unwrap();
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("hello"));

    // Typed values keep their type through the flags.
    cache.set(&key, 42i64, Ttl::secs(60)).unwrap();
    assert_eq!(cache.get::<i64>(&key).unwrap(), Some(42));
    cache.set(&key, &b"\x00\x01\xff"[..], Ttl::secs(60)).unwrap();
    assert_eq!(
        cache.get::<Vec<u8>>(&key).unwrap().as_deref(),
        Some(&b"\x00\x01\xff"[..])
    );

    cache.delete(&key).unwrap();
    assert_eq!(cache.get::<Vec<u8>>(&key).unwrap(), None);
    // Deleting a missing key is not an error.
    cache.delete(&key).unwrap();
}

#[test]
fn memcache_add_replace() {
    let cache = connect();
    let key = gen_random_key();

    assert!(!cache.replace(&key, "x", Ttl::secs(60)).unwrap());
    assert!(cache.add(&key, "first", Ttl::secs(60)).unwrap());
    assert!(!cache.add(&key, "second", Ttl::secs(60)).unwrap());
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("first"));
    assert!(cache.replace(&key, "third", Ttl::secs(60)).unwrap());
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("third"));
}

#[test]
fn memcache_many() {
    let cache = connect();
    let keys: Vec<String> = (0..5).map(|_| gen_random_key()).collect();
    let missing = gen_random_key();

    let pairs: Vec<(String, String)> = keys.iter().map(|k| (k.clone(), format!("v-{k}"))).collect();
    cache.set_many(pairs, Ttl::secs(60)).unwrap();

    let mut wanted: Vec<String> = keys.clone();
    wanted.push(missing.clone());
    let found: HashMap<String, String> = cache.get_many(wanted).unwrap();
    assert_eq!(found.len(), keys.len());
    for key in &keys {
        assert_eq!(found[key], format!("v-{key}"));
    }
    assert!(!found.contains_key(&missing));

    cache.delete_many(&keys).unwrap();
    let found: HashMap<String, String> = cache.get_many(keys.clone()).unwrap();
    assert!(found.is_empty());
}

#[test]
fn memcache_arithmetic_and_fragments() {
    let cache = connect();
    let key = gen_random_key();

    // incr on a missing key seeds it with the delta.
    assert_eq!(cache.incr(&key, 5, Ttl::secs(60)).unwrap(), 5);
    assert_eq!(cache.incr(&key, 3, Ttl::secs(60)).unwrap(), 8);
    assert_eq!(cache.decr(&key, 10, Ttl::secs(60)).unwrap(), 0);
    assert_eq!(cache.get::<u64>(&key).unwrap(), Some(0));

    let key = gen_random_key();
    cache.set(&key, "mid", Ttl::secs(60)).unwrap();
    cache.append(&key, b"-end", Ttl::secs(60)).unwrap();
    cache.prepend(&key, b"start-", Ttl::secs(60)).unwrap();
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("start-mid-end"));
}

#[test]
fn memcache_touch_and_inspect() {
    let cache = connect();
    let key = gen_random_key();

    assert!(cache.inspect(&key).unwrap().is_none());
    cache.set(&key, "abc", Ttl::secs(30)).unwrap();
    let info = cache.inspect(&key).unwrap().unwrap();
    assert_eq!(info.size, 3);
    let ttl = info.ttl.unwrap();
    assert!(
        ttl <= Duration::from_secs(30) && ttl > Duration::from_secs(20),
        "ttl {ttl:?}"
    );

    cache.touch(&key, Ttl::secs(600)).unwrap();
    let ttl = cache.inspect(&key).unwrap().unwrap().ttl.unwrap();
    assert!(ttl > Duration::from_secs(500), "ttl {ttl:?}");

    assert_eq!(
        cache.get_and_touch::<String>(&key, Ttl::NEVER).unwrap().as_deref(),
        Some("abc")
    );
    assert!(cache.inspect(&key).unwrap().unwrap().ttl.is_none());

    // Touching a missing key is not an error.
    cache.touch(gen_random_key(), Ttl::secs(60)).unwrap();
}

#[test]
fn memcache_update() {
    let cache = connect();
    let key = gen_random_key();

    let value = cache
        .update(&key, Ttl::secs(60), |cur: Option<i64>| cur.unwrap_or(0) + 1)
        .unwrap();
    assert_eq!(value, 1);
    let value = cache
        .update(&key, Ttl::secs(60), |cur: Option<i64>| cur.unwrap_or(0) + 1)
        .unwrap();
    assert_eq!(value, 2);
    assert_eq!(cache.get::<i64>(&key).unwrap(), Some(2));

    // A failing transform writes nothing.
    let err = cache
        .try_update(&key, Ttl::secs(60), |_: Option<i64>| Err::<i64, _>("nope"))
        .unwrap_err();
    assert!(matches!(err, Error::Callback(_)), "{err:?}");
    assert_eq!(cache.get::<i64>(&key).unwrap(), Some(2));
}

#[test]
fn memcache_update_is_atomic_under_contention() {
    let cache = Arc::new(connect());
    let key = Arc::new(gen_random_key());
    const THREADS: usize = 8;
    const ROUNDS: usize = 20;

    let conflicts = Arc::new(AtomicUsize::new(0));

    // The optimistic loop is bounded (UPDATE_ATTEMPTS) and surfaces
    // exhaustion as Error::Conflict; callers retry. What must hold is that
    // no increment is ever lost or applied twice.
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let key = Arc::clone(&key);
            let conflicts = Arc::clone(&conflicts);
            thread::spawn(move || {
                for _ in 0..ROUNDS {
                    loop {
                        match cache.update(&*key, Ttl::secs(60), |cur: Option<u64>| cur.unwrap_or(0) + 1) {
                            Ok(_) => break,
                            Err(Error::Conflict) => {
                                conflicts.fetch_add(1, Ordering::SeqCst);
                            }
                            Err(error) => panic!("{error:?}"),
                        }
                    }
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(cache.get::<u64>(&*key).unwrap(), Some((THREADS * ROUNDS) as u64));
    eprintln!(
        "update contention: {} Conflict retries",
        conflicts.load(Ordering::SeqCst)
    );
}

#[test]
fn memcache_take() {
    let cache = connect();
    let key = gen_random_key();

    assert_eq!(cache.take::<String>(&key).unwrap(), None);
    cache.set(&key, "once", Ttl::secs(60)).unwrap();
    assert_eq!(cache.take::<String>(&key).unwrap().as_deref(), Some("once"));
    assert_eq!(cache.take::<String>(&key).unwrap(), None);
    assert_eq!(cache.get::<String>(&key).unwrap(), None);
}

#[test]
fn memcache_fetch_computes_once() {
    let cache = connect();
    let key = gen_random_key();
    let calls = AtomicUsize::new(0);
    let loader = || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<String, std::convert::Infallible>("computed".into())
    };

    assert_eq!(cache.fetch(&key, Ttl::secs(60), loader).unwrap(), "computed");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // A hit never runs the loader.
    assert_eq!(cache.fetch(&key, Ttl::secs(60), loader).unwrap(), "computed");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("computed"));

    // A loader error surfaces and writes nothing.
    let key = gen_random_key();
    let err = cache
        .fetch(&key, Ttl::secs(60), || Err::<String, _>("boom"))
        .unwrap_err();
    assert!(matches!(err, Error::Callback(_)), "{err:?}");
    assert_eq!(cache.get::<String>(&key).unwrap(), None);
    // The failed lease does not block the next caller.
    assert_eq!(
        cache
            .fetch(&key, Ttl::secs(60), || Ok::<String, std::convert::Infallible>(
                "ok".into()
            ))
            .unwrap(),
        "ok"
    );
}

#[test]
fn memcache_fetch_shares_one_load_across_threads() {
    let cache = Arc::new(connect());
    let key = Arc::new(gen_random_key());
    let calls = Arc::new(AtomicUsize::new(0));
    const THREADS: usize = 8;

    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let key = Arc::clone(&key);
            let calls = Arc::clone(&calls);
            thread::spawn(move || {
                cache
                    .fetch(&*key, Ttl::secs(60), || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(200));
                        Ok::<String, std::convert::Infallible>("shared".into())
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
fn memcache_invalidate_then_fetch_recomputes() {
    let cache = connect();
    let key = gen_random_key();

    cache.set(&key, "old", Ttl::secs(60)).unwrap();
    cache.invalidate(&key, Duration::from_secs(30)).unwrap();
    // Plain reads keep serving the stale value; update and take see a miss.
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("old"));
    assert_eq!(cache.take::<String>(&key).unwrap(), None);
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("old"));
    let seen = cache
        .update(&key, Ttl::secs(60), |cur: Option<String>| {
            cur.unwrap_or_else(|| "miss".into())
        })
        .unwrap();
    assert_eq!(seen, "miss");
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("miss"));

    // Back to a stale item for the fetch path.
    cache.set(&key, "old", Ttl::secs(60)).unwrap();
    cache.invalidate(&key, Duration::from_secs(30)).unwrap();

    let calls = AtomicUsize::new(0);
    let value = cache
        .fetch(&key, Ttl::secs(60), || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<String, std::convert::Infallible>("new".into())
        })
        .unwrap();
    assert_eq!(value, "new");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(cache.get::<String>(&key).unwrap().as_deref(), Some("new"));

    // Invalidating a missing key is not an error.
    cache.invalidate(gen_random_key(), Duration::from_secs(1)).unwrap();
}

#[test]
fn memcache_multi_server() {
    let cache = Memcache::connect(["localhost:12345", "localhost:12346"]).unwrap();
    let keys: Vec<String> = (0..20).map(|_| gen_random_key()).collect();
    let pairs: Vec<(String, String)> = keys.iter().map(|k| (k.clone(), k.clone())).collect();
    cache.set_many(pairs, Ttl::secs(60)).unwrap();
    let found: HashMap<String, String> = cache.get_many(keys.clone()).unwrap();
    assert_eq!(found.len(), keys.len());
    for key in &keys {
        assert_eq!(cache.get::<String>(key).unwrap().as_deref(), Some(key.as_str()));
    }
    cache.delete_many(&keys).unwrap();
}

#[test]
fn memcache_builder_and_meta_access() {
    let cache = Memcache::builder()
        .timeout(Duration::from_secs(2))
        .max_idle(2)
        .connect([SERVER])
        .unwrap();
    let key = gen_random_key();
    cache.set(&key, "via-builder", Ttl::secs(60)).unwrap();
    // The escape hatch sees what the scenario layer wrote.
    let raw = cache.meta().get(&*key).send().unwrap();
    assert_eq!(raw.value.as_deref(), Some(&b"via-builder"[..]));
}

#[cfg(feature = "serde_json")]
#[test]
fn memcache_json_values() {
    use memcache::exp::Json;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Profile {
        name: String,
        age: u32,
    }

    let cache = connect();
    let key = gen_random_key();
    let profile = Profile {
        name: "ann".into(),
        age: 30,
    };
    cache.set(&key, Json(profile.clone()), Ttl::secs(60)).unwrap();
    let got: Json<Profile> = cache.get(&key).unwrap().unwrap();
    assert_eq!(got.0, profile);

    let got: Json<Profile> = cache
        .update(&key, Ttl::secs(60), |cur: Option<Json<Profile>>| {
            let mut p = cur.unwrap().0;
            p.age += 1;
            Json(p)
        })
        .unwrap();
    assert_eq!(got.age, 31);
}

#[cfg(feature = "tokio")]
mod async_tests {
    use super::*;
    use memcache::exp::AsyncMemcache;

    async fn connect_async() -> AsyncMemcache {
        AsyncMemcache::connect([SERVER]).await.unwrap()
    }

    #[tokio::test]
    async fn async_memcache_roundtrip() {
        let cache = connect_async().await;
        let key = gen_random_key();

        assert_eq!(cache.get::<String>(&key).await.unwrap(), None);
        cache.set(&key, "hello", Ttl::secs(60)).await.unwrap();
        assert_eq!(cache.get::<String>(&key).await.unwrap().as_deref(), Some("hello"));
        assert!(!cache.add(&key, "x", Ttl::secs(60)).await.unwrap());
        assert!(cache.replace(&key, "world", Ttl::secs(60)).await.unwrap());
        assert_eq!(
            cache
                .get_and_touch::<String>(&key, Ttl::secs(120))
                .await
                .unwrap()
                .as_deref(),
            Some("world")
        );
        let info = cache.inspect(&key).await.unwrap().unwrap();
        assert_eq!(info.size, 5);
        assert!(info.ttl.unwrap() > Duration::from_secs(60));

        cache.append(&key, b"!", Ttl::secs(60)).await.unwrap();
        assert_eq!(cache.take::<String>(&key).await.unwrap().as_deref(), Some("world!"));
        assert_eq!(cache.get::<String>(&key).await.unwrap(), None);

        assert_eq!(cache.incr(&key, 7, Ttl::secs(60)).await.unwrap(), 7);
        assert_eq!(cache.decr(&key, 2, Ttl::secs(60)).await.unwrap(), 5);
        cache.delete(&key).await.unwrap();
    }

    #[tokio::test]
    async fn async_memcache_many() {
        let cache = connect_async().await;
        let keys: Vec<String> = (0..5).map(|_| gen_random_key()).collect();
        let pairs: Vec<(String, i64)> = keys.iter().enumerate().map(|(i, k)| (k.clone(), i as i64)).collect();
        cache.set_many(pairs, Ttl::secs(60)).await.unwrap();
        let found: HashMap<String, i64> = cache.get_many(keys.clone()).await.unwrap();
        assert_eq!(found.len(), keys.len());
        for (i, key) in keys.iter().enumerate() {
            assert_eq!(found[key], i as i64);
        }
        cache.delete_many(&keys).await.unwrap();
        let found: HashMap<String, i64> = cache.get_many(keys).await.unwrap();
        assert!(found.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn async_memcache_update_under_contention() {
        let cache = Arc::new(connect_async().await);
        let key = Arc::new(gen_random_key());
        const TASKS: usize = 8;
        const ROUNDS: usize = 20;

        let mut handles = Vec::new();
        for _ in 0..TASKS {
            let cache = Arc::clone(&cache);
            let key = Arc::clone(&key);
            handles.push(tokio::spawn(async move {
                for _ in 0..ROUNDS {
                    loop {
                        match cache
                            .update(&*key, Ttl::secs(60), |cur: Option<u64>| cur.unwrap_or(0) + 1)
                            .await
                        {
                            Ok(_) => break,
                            Err(Error::Conflict) => {}
                            Err(error) => panic!("{error:?}"),
                        }
                    }
                }
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert_eq!(cache.get::<u64>(&*key).await.unwrap(), Some((TASKS * ROUNDS) as u64));

        let err = cache
            .try_update(&*key, Ttl::secs(60), |_: Option<u64>| async { Err::<u64, _>("nope") })
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Callback(_)), "{err:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn async_memcache_fetch_computes_once() {
        let cache = Arc::new(connect_async().await);
        let key = Arc::new(gen_random_key());
        let calls = Arc::new(AtomicUsize::new(0));
        const TASKS: usize = 8;

        let mut handles = Vec::new();
        for _ in 0..TASKS {
            let cache = Arc::clone(&cache);
            let key = Arc::clone(&key);
            let calls = Arc::clone(&calls);
            handles.push(tokio::spawn(async move {
                cache
                    .fetch(&*key, Ttl::secs(60), async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        Ok::<String, std::convert::Infallible>("shared".into())
                    })
                    .await
                    .unwrap()
            }));
        }
        for handle in handles {
            assert_eq!(handle.await.unwrap(), "shared");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // A hit drops the loader unpolled.
        let value = cache
            .fetch(&*key, Ttl::secs(60), async {
                panic!("loader must not run on a hit");
                #[allow(unreachable_code)]
                Ok::<String, std::convert::Infallible>(String::new())
            })
            .await
            .unwrap();
        assert_eq!(value, "shared");

        // Invalidate then fetch: the async path serves the stale value at
        // once and recomputes in the background. A plain get on a stale
        // item also wins the token and returns it on a detached task, so a
        // fetch racing that return may see the token busy and serve stale
        // without refreshing. Poll with fetch until the refresh lands.
        cache.invalidate(&*key, Duration::from_secs(30)).await.unwrap();
        assert_eq!(cache.get::<String>(&*key).await.unwrap().as_deref(), Some("shared"));
        let mut landed = None;
        for _ in 0..50 {
            let value = cache
                .fetch(&*key, Ttl::secs(60), async {
                    Ok::<String, std::convert::Infallible>("fresh".into())
                })
                .await
                .unwrap();
            if value == "fresh" {
                landed = Some(value);
                break;
            }
            assert_eq!(value, "shared");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(landed.as_deref(), Some("fresh"));
        assert_eq!(cache.get::<String>(&*key).await.unwrap().as_deref(), Some("fresh"));
        let info = cache.inspect(&*key).await.unwrap().unwrap();
        assert!(info.ttl.unwrap() > Duration::from_secs(30), "ttl {:?}", info.ttl);
    }
}
