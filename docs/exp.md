# High-level client guide (Experimental)

> **Experimental.** The high-level client lives under `memcache::exp` and its API
> may change in any minor release. If you depend on it, pin the **minor version**
> in `Cargo.toml`. Patch releases (`x.y.Z`) will not introduce breaking changes,
> but minor releases (`x.Y.0`) might.
>
> ```toml
> [dependencies]
> memcache = "0.20"   # allows 0.20.x, blocks 0.21+
> ```

`memcache::exp::Memcache` hides the [meta protocol](https://github.com/memcached/memcached/blob/master/doc/protocol.txt) behind verbs named for what you are doing. The protocol's CAS tokens and leases never surface in caller code. Instead of reading a version and writing it back, call `update` with a transform closure and the client runs the read, compare and swap, retry loop internally. Instead of building dogpile protection, call `fetch` with a loader and the client makes sure the value is computed once. When you do need the raw protocol, every meta command is still reachable through `cache.meta()`.

## Creating a client

```rust ignore
Memcache::builder()
    .timeout(Duration::from_secs(1))          // per-exchange deadline, None removes it
    .connect_timeout(Duration::from_secs(1))  // limit on dialing a server
    .max_idle(8)                              // idle connections retained per server
    .max_idle_age(Duration::from_secs(90))    // do not reuse a connection idle longer than this
    .max_connections(None)                    // cap on connections in use per server
    .degrade(false)                           // failure policy, see below
    .on_error(|event| { /* observability hook */ })
    .router(Rendezvous::default())            // or hash_function(..) to keep rendezvous with another hash
    .connect(["cache1:11211", "cache2:11211"])?;
```

```rust ignore
use memcache::exp::Memcache;

let cache = Memcache::connect(["cache1:11211", "cache2:11211"])?;
```

With multiple servers, keys are distributed by rendezvous hashing; `hash_function` swaps the hash and `router` swaps the whole placement strategy through the `Router` trait. Each server has an elastic connection pool: `max_idle` limits retained idle connections, not active requests, and `max_connections` caps the latter if you need a bound. Addresses are resolved at connect time and connections dialed lazily. The client is cheap to clone and every clone shares the same pools.

Keys are anything `AsRef<[u8]>`, so `&str`, `String`, `&[u8]` and `Vec<u8>` all name the same item.

## Values

Values are business objects and the encoding lives on the value type through the `Encode` and `Decode` traits. Strings, byte slices, vectors and the integer types are built in; `Json<T>` wraps any `serde` type behind the `serde_json` feature; and you can implement the traits for your own types. The requested type drives decoding, so a read always says what it expects:

```rust ignore
use memcache::exp::Json;

cache.set("user:1", Json(&user), Ttl::secs(600))?;
let user: Option<Json<User>> = cache.get("user:1")?;
```

A value that encodes to zero bytes is rejected with `Error::EmptyValue`, because memcached represents lease placeholders as zero byte items; every read folds such an item into a miss.

## Reading

```rust ignore
cache.get::<T>(key)?                 // -> Option<T>
cache.get_and_touch::<T>(key, ttl)?      // -> Option<T>, slides the expiry on a hit
cache.get_many::<T, _>(keys)?        // -> HashMap<K, T>, hits only
cache.inspect(key)?                  // -> Option<ItemInfo>
```

`get` reads one value. A miss is a normal answer, not an error: it returns `Ok(None)`, and `Err` is reserved for infrastructure failure. The two never mix.

```rust ignore
let user: Option<User> = cache.get(format!("user:{uid}"))?;
let user = match user {
    Some(user) => user,
    None => {
        let user = db.load_user(uid)?;
        cache.set(format!("user:{uid}"), &user, Ttl::secs(600))?;
        user
    }
};
```

`get_many` reads a set of keys in one round trip per server and returns the hits, keyed by the very key values the caller passed. A miss is expressed by key absence.

`get_and_touch` makes the same protocol command also slide the hit's expiration, which turns a read into the read half of session renewal:

```rust ignore
let session: Option<Session> = cache.get_and_touch(format!("session:{sid}"), Ttl::secs(1800))?;
```

The slide is memcached's native touch and is blind: it extends whatever the read hits, including a value kept stale by `invalidate`. A revocation that must stick goes through `delete`.

`inspect` returns an item's metadata (remaining ttl, size, last access, whether it was ever hit) without transferring the value or bumping its LRU position. It is an observability tool for debugging; branching business logic on metadata is inherently racy and not a supported pattern.

## Writing

```rust ignore
cache.set(key, value, ttl)?          // -> ()
cache.set_many(pairs, ttl)?          // -> ()
cache.add(key, value, ttl)?          // -> bool, true when this call won
cache.replace(key, value, ttl)?      // -> bool, never resurrects
cache.touch(key, ttl)?               // -> ()
cache.delete(key)?                   // -> ()
cache.invalidate(key, grace)?        // -> ()
cache.delete_many(keys)?             // -> ()
```

`set` unconditionally stores a value for `ttl`. `set_many` stores a batch in one round trip per server, all sharing the same ttl.

Every storing method takes a required `Ttl`, with no client wide default. `Ttl::secs(n)` and a `Duration` are durations from now, `Ttl::at(SystemTime)` is the absolute moment of expiry, and `Ttl::NEVER` stores without expiration, spelling that choice out at the call site. A zero duration is not "never" and is rejected as `Error::Usage`; a duration above 30 days is sent as an absolute timestamp, as the protocol demands.

`add` stores only if the key is absent and reports whether this call won, which is a distributed claim:

```rust ignore
if cache.add("job:daily", "1", Ttl::secs(86400))? {
    run_daily_report();
}
```

`replace` stores only if the key exists. It never resurrects a key that has been deleted in between, which makes it the write half of session handling: a revoked session stays revoked even if a request that loaded it earlier writes it back later.

```rust ignore
cache.replace(format!("session:{sid}"), &session, Ttl::secs(1800))?;
```

`touch` extends a key's ttl without transferring its value, as one blind protocol command. It exists for large values (rendered pages, serialized reports) where reading the payload back just to renew it wastes bandwidth; when you are reading anyway, use `get_and_touch`.

`delete` erases a key outright and the next reader pays a full miss; a missing key is not an error. `invalidate` marks the value stale for a grace period instead:

```rust ignore
cache.delete(format!("article:{aid}"))?;                               // hard: old data must not reappear
cache.invalidate(format!("article:{aid}"), Duration::from_secs(60))?;  // soft: readers keep the old copy briefly
```

During the grace window plain readers keep getting the old copy, while a `fetch` elects one caller to recompute; afterwards the key decays into a normal miss. Soft invalidation pairs with `fetch` managed keys, and the grace bound holds only while nothing renews the key, since a touch slides it like any other expiration. Use the hard form when the old value must not be served for even a second.

## Get or compute

```rust ignore
cache.fetch(key, freshness, loader)?   // -> T
```

The highest frequency cache pattern as one verb: `get` with a static fallback on the caller's side, `fetch` with a loader that computes the value and writes it back. `freshness` is a `Ttl`, optionally carrying a refresh-ahead window through `Ttl::refresh_ahead`; only `fetch` accepts a `Freshness`, so the modifier cannot leak into plain writes, and a wrong combination (a window on `Ttl::NEVER`, a window wider than the ttl) is an `Error::Usage` at the call site instead of being silently ignored.

```rust ignore
let report: Report = cache.fetch("report:q3", Ttl::secs(3600), || build_report())?;
```

On a miss, one caller across all processes wins a server side lease and runs the loader. Other callers in the same process wait on that result, and other processes wait briefly then compute locally without writing back. So a hot key expiring under a thousand concurrent requests costs one recomputation, not a thousand.

With `refresh_ahead`, a value whose remaining ttl has entered the window is served as is while one elected caller recomputes, so the curve never shows an expiry spike:

```rust ignore
let feed: Feed = cache.fetch(
    "home:feed",
    Ttl::secs(300).refresh_ahead(Duration::from_secs(30)),
    || build_feed(),
)?;
```

The synchronous client's elected winner recomputes in place and pays one recomputation latency (the library owns no threads, so who pays what stays predictable). The async client's winner returns the current value immediately and recomputes on a background task owned by the client, so no request pays the refresh latency.

Every write back is conditional on the version observed at election, so a key deleted mid recompute is never resurrected. Write back failures never change what `fetch` returns; they go to the `on_error` hook. A `fetch` never fails because coordination failed: every path ends in a value or the loader's own error, carried out as `Error::Callback` and recoverable with `Error::callback::<E>()`.

## Atomic modification

```rust ignore
cache.update(key, ttl, |current| ..)?       // -> new value
cache.try_update(key, ttl, |current| ..)?   // -> new value, closure may fail
```

`update` atomically transforms a value. It reads the current value with its version, applies the closure, writes back only if nothing changed in between, and retries on conflict. The closure receives `Option<T>`, `None` on a miss, so the starting value is its decision. It may run multiple times, so it must be pure. `try_update` takes a fallible closure: an `Err` aborts the call, the entry is left unwritten and the error surfaces as `Error::Callback`. If the retry loop keeps losing to concurrent writers, the call fails with `Error::Conflict`. A value kept stale by `invalidate` counts as a miss, because transforming invalidated data would silently launder it back to fresh.

```rust ignore
let cart: Json<Cart> = cache.update("cart:42", Ttl::secs(1800), |current| {
    let mut cart = current.map(Json::into_inner).unwrap_or_default();
    cart.items.push(item.clone());
    Json(cart)
})?;
```

```rust ignore
cache.incr(key, delta, ttl)?   // -> u64
cache.decr(key, delta, ttl)?   // -> u64
```

`incr` adds `delta` to a counter and returns the new value, creating the counter on a miss so the first request counts as `delta`. `decr` subtracts and saturates at zero. Since the ttl is fixed at creation and later calls never extend it, this is exactly fixed window rate limiting:

```rust ignore
if cache.incr(format!("rate:{ip}"), 1, Ttl::secs(60))? > 100 {
    return Err(TooManyRequests);
}
```

```rust ignore
cache.append(key, fragment, ttl)?   // -> ()
cache.prepend(key, fragment, ttl)?  // -> ()
cache.take::<T>(key)?               // -> Option<T>, atomic take and delete
```

`append` and `prepend` concatenate raw bytes onto a value, creating it on a miss; the ttl applies only to that creation, so later calls never extend the buffer's life. They bypass `Encode` because this key family's value model is a delimited byte stream, not an object. `take` atomically reads a value and deletes it, with no window in which concurrently appended bytes can be lost. Together they make a collect then drain pattern, such as buffering events per user and periodically taking the batch:

```rust ignore
cache.append(format!("events:{uid}"), b"login;", Ttl::secs(86400))?;
let buffered: Option<Vec<u8>> = cache.take(format!("events:{uid}"))?;   // split by the caller
```

`take` is not limited to byte streams; taking a one-time token stored with `set` works the same way.

## Batches

```rust ignore
let found: HashMap<&str, Page> = cache.get_many(["a", "b", "c"])?;
cache.set_many([("a", &page_a), ("b", &page_b)], Ttl::secs(300))?;
cache.delete_many(["a", "b", "c"])?;
```

The `_many` verbs group their keys per server and run one round trip per server. Every value is encoded before anything is written, so an encoding error leaves the cache untouched. Without `degrade`, every server group runs and the first failing one fails the whole call; the groups that succeeded are not rolled back. With `degrade`, a failing server only removes its own keys from a `get_many` result, and its writes are dropped silently.

Independent operations on different keys have no high-level pipeline: on the sync client issue them in sequence, on the async client run them concurrently with `tokio::join!`, and for one round trip across mixed commands drop to the protocol layer's `run_batch`.

## Failure policy

By default every infrastructure failure surfaces as an `Error` (`Io`, `Timeout`, `Protocol`, `Server`, ...). The `degrade(true)` constructor policy decouples a cache outage from a site outage:

```rust ignore
let cache = Memcache::builder()
    .degrade(true)
    .on_error(|event| metrics.count(event.op, &event.kind))
    .connect(servers)?;
```

Under degrade, reads report failures as misses, a `fetch` computes locally without writing back, and blind writes (`set`, `delete`, `invalidate`, `touch`, `append`, ...) give up silently. Verbs whose answer feeds a business decision (`add`, `replace`, `incr`, `decr`, `update`, `take`) keep failing loudly even under degrade, because inventing an answer is worse than failing. `Error::Ambiguous` (the request was written and the outcome is unknown, so the write may have landed) always surfaces: degrading covers "the cache is down", never "the write may or may not have happened". Every absorbed failure still reaches the `on_error` hook as an `ErrorEvent` carrying the verb, the kind of absorption and the error, so degrading business behavior never degrades observability. The client never automatically retries a command after writing begins, since blindly retrying arithmetic or append could apply the mutation twice; `Error::is_retryable` tells you when a request was definitely not applied.

## Async client

`AsyncMemcache`, behind the `tokio` feature, is the same table of verbs plus `.await`; `fetch` takes a future instead of a closure and `try_update` takes an async closure.

```rust ignore
use memcache::exp::{AsyncMemcache, Ttl};

let cache = AsyncMemcache::connect(["127.0.0.1:11211"]).await?;
let report: Report = cache
    .fetch("report:q3", Ttl::secs(3600), async move { build_report(db).await })
    .await?;
let (user, hits) = tokio::try_join!(
    cache.get::<User>(format!("user:{uid}")),
    cache.incr(format!("rate:{ip}"), 1, Ttl::secs(60)),
)?;
```

The loader runs on a detached task owned by the client: on a miss its result is shared by every same-process caller and survives any one of them being cancelled, and inside a refresh-ahead window or an `invalidate` grace period the current value is returned at once while the loader recomputes in the background. A hit drops the loader unpolled. Every verb returns a `Send` future, so it can be held across `tokio::spawn` and `try_join!`.

## Protocol access

Everything the high-level verbs do not cover lives behind `cache.meta()` (or a standalone `MetaClient` / `AsyncMetaClient`), a 1:1 typed mapping of the meta protocol: `get` / `set` / `delete` / `increment` / `decrement` (`mg` / `ms` / `md` / `ma`) with one builder method per protocol flag. It works on raw bytes plus client flags and returns lightly parsed results that report miss, CAS mismatch and lease state as values, without encoding or semantic mapping.

```rust ignore
use memcache::exp::{Get, Meta, Set, Ttl};

let client = cache.meta();
let stored = client.set("key", b"payload").ttl(Ttl::secs(60)).return_cas().send()?;
let got = client.get("key").meta(Meta::NONE.cas().ttl()).send()?;
assert_eq!(got.item.cas, stored.cas);

// Several operations in one round trip per server, one result each.
let results = client.run_batch(vec![
    Set::new("a", "1").ttl(Ttl::secs(60)).into(),
    Get::new("b").into(),
])?;

// Leases, CAS and stale flags are all there for hand-rolled flows.
let read = client.get("hot").lease_ttl(30).refresh_before(10).send()?;
if read.won_lease() {
    client.set("hot", recompute()).ttl(Ttl::secs(300)).compare_cas(read.item.cas.unwrap()).send()?;
}
```

Below that, `MetaCommand` / `MetaResponse` and the `build_*` / `parse_*` functions expose the wire layer for anything the typed operations do not cover. See the `memcache::exp` module docs for the full verb table and the protocol layer reference.
