//! The blocking scenario client: one verb per caching scenario, business
//! values in and out, coordination and protocol state kept inside.

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::net::ToSocketAddrs;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::client::{MetaClient, MetaClientBuilder};
use super::core::scenario::{
    FetchStep, ItemInfo, ReadView, StaleWin, TakeStep, UPDATE_ATTEMPTS, WAIT_BACKOFF, encode_value, fetch_step,
    finish_concat, finish_counter, finish_erase, finish_get, finish_inspect, finish_store, grace_ttl, plan_concat,
    plan_counter, plan_election, plan_erase, plan_get, plan_inspect, plan_probe, plan_release_lease, plan_return_win,
    plan_store, plan_touch, plan_write_back, read_view, take_step, update_step,
};
use super::error::{Error, Result};
use super::meta_api::{MetaCommandResult, SetMode};
use super::meta_command::MetaCommand;
use super::ttl::{Freshness, Ttl};
use super::value::{Decode, Encode, Encoded};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Why an [`ErrorEvent`] was raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A verb absorbed an infrastructure failure because `degrade` is on.
    Degraded,
    /// A background loader (tokio `fetch` refresh) failed.
    BackgroundLoader,
    /// A `fetch` write-back failed or was abandoned because the item
    /// changed during recomputation.
    WriteBack,
    /// Handing a stale-recache token back to the server failed.
    LeaseReturn,
}

/// A failure that never reaches a caller, delivered to the `on_error`
/// hook: the only visibility into degraded calls and background work.
#[non_exhaustive]
pub struct ErrorEvent<'a> {
    pub kind: ErrorKind,
    /// The verb that raised the event (`"get"`, `"fetch"`, ...).
    pub op: &'static str,
    pub key: &'a [u8],
    pub error: &'a Error,
}

impl fmt::Debug for ErrorEvent<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ErrorEvent")
            .field("kind", &self.kind)
            .field("op", &self.op)
            .field("key", &String::from_utf8_lossy(self.key))
            .field("error", &self.error)
            .finish()
    }
}

pub(crate) type ErrorHook = Arc<dyn Fn(&ErrorEvent<'_>) + Send + Sync>;

/// Failure policy shared by both scenario clients.
#[derive(Clone, Default)]
pub(crate) struct Policy {
    pub(crate) degrade: bool,
    pub(crate) on_error: Option<ErrorHook>,
}

impl Policy {
    pub(crate) fn report(&self, kind: ErrorKind, op: &'static str, key: &[u8], error: &Error) {
        if let Some(hook) = &self.on_error {
            hook(&ErrorEvent { kind, op, key, error });
        }
    }

    /// Whether degrade mode swallows this failure. Only "the cache is
    /// unavailable" degrades: a written request with an unknown outcome
    /// and caller bugs always surface.
    pub(crate) fn absorbs(&self, error: &Error) -> bool {
        self.degrade
            && matches!(
                error,
                Error::Io(_) | Error::Timeout { .. } | Error::Protocol(_) | Error::Server(_)
            )
    }

    /// Resolve a verb's outcome under the policy: an absorbed failure is
    /// reported and replaced by `fallback`.
    pub(crate) fn settle<T>(
        &self,
        op: &'static str,
        key: &[u8],
        result: Result<T>,
        fallback: impl FnOnce() -> T,
    ) -> Result<T> {
        match result {
            Err(error) if self.absorbs(&error) => {
                self.report(ErrorKind::Degraded, op, key, &error);
                Ok(fallback())
            }
            other => other,
        }
    }
}

/// Configures a [`Memcache`] before connecting.
///
/// ```no_run
/// use std::time::Duration;
/// use memcache::exp::Memcache;
///
/// let cache = Memcache::builder()
///     .timeout(Duration::from_millis(500))
///     .degrade(true)
///     .on_error(|event| eprintln!("{event:?}"))
///     .connect(["cache1:11211", "cache2:11211"])
///     .unwrap();
/// ```
#[derive(Clone, Default)]
pub struct MemcacheBuilder {
    pub(crate) meta: MetaClientBuilder,
    pub(crate) policy: Policy,
}

impl MemcacheBuilder {
    pub fn new() -> MemcacheBuilder {
        MemcacheBuilder::default()
    }

    /// One deadline per exchange (a command or a batch, write plus reads),
    /// default one second; `None` removes the limit.
    pub fn timeout(mut self, timeout: impl Into<Option<Duration>>) -> MemcacheBuilder {
        self.meta = self.meta.io_timeout(timeout.into());
        self
    }

    /// Limit on dialing a server, default one second; `None` removes it.
    pub fn connect_timeout(mut self, timeout: impl Into<Option<Duration>>) -> MemcacheBuilder {
        self.meta = self.meta.connect_timeout(timeout.into());
        self
    }

    /// Idle connections retained per server (default 8).
    pub fn max_idle(mut self, max_idle: usize) -> MemcacheBuilder {
        self.meta = self.meta.max_idle(max_idle);
        self
    }

    /// Replace the key hash used for routing.
    pub fn hash_function(mut self, hash_function: fn(&[u8]) -> u64) -> MemcacheBuilder {
        self.meta = self.meta.hash_function(hash_function);
        self
    }

    /// Treat a cache outage as a miss instead of an error (default off).
    /// Reads report a miss, `fetch` runs its loader locally, blind writes
    /// are dropped; verbs whose answer feeds a business decision (`add`,
    /// `replace`, `update`, `take`, `incr`, `decr`) still fail, and so does
    /// an [`Ambiguous`](Error::Ambiguous) write. Absorbed failures reach
    /// [`on_error`](Self::on_error).
    pub fn degrade(mut self, degrade: bool) -> MemcacheBuilder {
        self.policy.degrade = degrade;
        self
    }

    /// Receive failures that never reach a caller (see [`ErrorEvent`]).
    /// Without a hook they are dropped.
    pub fn on_error(mut self, hook: impl Fn(&ErrorEvent<'_>) + Send + Sync + 'static) -> MemcacheBuilder {
        self.policy.on_error = Some(Arc::new(hook));
        self
    }

    /// Connect to one or more servers. Addresses are resolved now and
    /// connections dialed lazily.
    pub fn connect<A: ToSocketAddrs>(self, addrs: impl IntoIterator<Item = A>) -> Result<Memcache> {
        Ok(Memcache {
            meta: self.meta.connect_multiple(addrs)?,
            shared: Arc::new(Shared {
                policy: self.policy,
                flights: Mutex::new(HashMap::new()),
            }),
        })
    }
}

struct Shared {
    policy: Policy,
    /// In-process singleflight: one loader per key at a time, the rest
    /// share its encoded result.
    flights: Mutex<HashMap<Vec<u8>, Arc<Flight>>>,
}

/// A pending loader result shared by every same-process `fetch` of a key.
struct Flight {
    state: Mutex<Option<Result<Arc<Encoded>>>>,
    ready: Condvar,
}

impl Flight {
    fn new() -> Arc<Flight> {
        Arc::new(Flight {
            state: Mutex::new(None),
            ready: Condvar::new(),
        })
    }

    fn wait(&self) -> Result<Arc<Encoded>> {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(result) = state.as_ref() {
                return match result {
                    Ok(encoded) => Ok(Arc::clone(encoded)),
                    Err(error) => Err(error.duplicate()),
                };
            }
            state = self.ready.wait(state).unwrap();
        }
    }

    fn finish(&self, result: Result<Arc<Encoded>>) {
        *self.state.lock().unwrap() = Some(result);
        self.ready.notify_all();
    }
}

#[derive(Debug)]
struct LoaderPanicked;

impl fmt::Display for LoaderPanicked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fetch loader panicked")
    }
}

impl std::error::Error for LoaderPanicked {}

/// The leader's handle on its flight: publishes the result, and if the
/// leader unwinds first, fails the flight so waiters never block forever.
struct Lead<'a> {
    shared: &'a Shared,
    key: &'a [u8],
    flight: Arc<Flight>,
    done: bool,
}

impl Lead<'_> {
    fn finish(mut self, result: Result<Arc<Encoded>>) {
        self.done = true;
        self.shared.forget(self.key, &self.flight);
        self.flight.finish(result);
    }
}

impl Drop for Lead<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.shared.forget(self.key, &self.flight);
            self.flight.finish(Err(Error::Callback(Arc::new(LoaderPanicked))));
        }
    }
}

impl Shared {
    /// Join the flight for `key`, or start one: the boolean says whether
    /// the caller is its leader.
    fn claim(&self, key: &[u8]) -> (Arc<Flight>, bool) {
        let mut flights = self.flights.lock().unwrap();
        if let Some(flight) = flights.get(key) {
            return (Arc::clone(flight), false);
        }
        let flight = Flight::new();
        flights.insert(key.to_vec(), Arc::clone(&flight));
        (flight, true)
    }

    fn existing(&self, key: &[u8]) -> Option<Arc<Flight>> {
        self.flights.lock().unwrap().get(key).cloned()
    }

    fn forget(&self, key: &[u8], flight: &Arc<Flight>) {
        let mut flights = self.flights.lock().unwrap();
        if flights.get(key).is_some_and(|current| Arc::ptr_eq(current, flight)) {
            flights.remove(key);
        }
    }
}

fn callback_error(error: impl Into<BoxError>) -> Error {
    Error::Callback(Arc::from(error.into()))
}

fn decode_shared<T: Decode>(encoded: Arc<Encoded>) -> Result<T> {
    Ok(T::decode(encoded.bytes.clone(), encoded.flags)?)
}

/// A blocking memcached client organized by scenario.
///
/// Every verb returns a business value: a miss is `Ok(None)` or an absent
/// map key, never an error; conditional writes answer with `bool`; `fetch`
/// and `update` consume the miss entirely. Values go through [`Encode`] /
/// [`Decode`] on the value type; counters (`incr` / `decr`) and byte
/// streams (`append` / `prepend`) bypass them. Every stored value names
/// its lifetime as a [`Ttl`]. The protocol layer stays reachable through
/// [`meta`](Self::meta).
///
/// The client is cheap to clone; clones share the connection pools, the
/// failure policy and the in-process singleflight table.
///
/// Zero-byte values are reserved as lease placeholders: writes of a value
/// that encodes to nothing fail with [`Error::EmptyValue`], and reads fold
/// a zero-byte item into a miss.
///
/// ```no_run
/// use memcache::exp::{Memcache, Ttl};
///
/// let cache = Memcache::connect(["127.0.0.1:11211"]).unwrap();
/// cache.set("greeting", "hello", Ttl::secs(60)).unwrap();
/// let greeting: Option<String> = cache.get("greeting").unwrap();
/// let report: String = cache
///     .fetch("report", Ttl::secs(300), || Ok::<_, std::io::Error>(String::from("computed")))
///     .unwrap();
/// ```
#[derive(Clone)]
pub struct Memcache {
    meta: MetaClient,
    shared: Arc<Shared>,
}

impl fmt::Debug for Memcache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Memcache")
            .field("degrade", &self.shared.policy.degrade)
            .finish_non_exhaustive()
    }
}

impl Memcache {
    /// Connect with the default configuration; see [`builder`](Self::builder).
    pub fn connect<A: ToSocketAddrs>(addrs: impl IntoIterator<Item = A>) -> Result<Memcache> {
        MemcacheBuilder::new().connect(addrs)
    }

    pub fn builder() -> MemcacheBuilder {
        MemcacheBuilder::new()
    }

    /// The protocol layer: every meta command with every option, for what
    /// the scenario verbs do not cover.
    pub fn meta(&self) -> &MetaClient {
        &self.meta
    }

    fn policy(&self) -> &Policy {
        &self.shared.policy
    }

    fn exchange(&self, key: &[u8], command: &MetaCommand) -> Result<MetaCommandResult> {
        self.meta.exchange(key, command)
    }

    /// Hand an accidentally won stale-recache token back, inline.
    fn return_win(&self, op: &'static str, key: &[u8], win: StaleWin) {
        let outcome = plan_return_win(key, win).and_then(|command| self.exchange(key, &command));
        if let Err(error) = outcome {
            self.policy().report(ErrorKind::LeaseReturn, op, key, &error);
        }
    }

    /// A scenario read: run it, return any stale token it won.
    fn read(&self, op: &'static str, key: &[u8], command: &MetaCommand) -> Result<ReadView> {
        let view = read_view(self.exchange(key, command)?)?;
        if let Some(win) = view.stale_win() {
            self.return_win(op, key, win);
        }
        Ok(view)
    }

    /// Read a value; `Ok(None)` on a miss.
    pub fn get<T: Decode>(&self, key: impl AsRef<[u8]>) -> Result<Option<T>> {
        let key = key.as_ref();
        let result = plan_get(key, None).and_then(|command| self.read("get", key, &command));
        self.policy()
            .settle("get", key, result, ReadView::default)
            .and_then(finish_get)
    }

    /// Read a value and slide its expiry to `ttl` in the same command. The
    /// touch is blind: it also extends an item kept stale by
    /// [`invalidate`](Self::invalidate), so sliding-expiry keys (sessions)
    /// are revoked with [`delete`](Self::delete).
    pub fn get_touch<T: Decode>(&self, key: impl AsRef<[u8]>, ttl: impl Into<Ttl>) -> Result<Option<T>> {
        let key = key.as_ref();
        let result = plan_get(key, Some(ttl.into())).and_then(|command| self.read("get_touch", key, &command));
        self.policy()
            .settle("get_touch", key, result, ReadView::default)
            .and_then(finish_get)
    }

    /// Read several keys, one round trip per server; hits only, keyed by
    /// the keys passed in. Without `degrade` the first failing server
    /// group fails the call after every group ran; with it, a failing
    /// server only removes its own keys.
    pub fn get_many<T, K>(&self, keys: impl IntoIterator<Item = K>) -> Result<HashMap<K, T>>
    where
        T: Decode,
        K: AsRef<[u8]> + Hash + Eq,
    {
        let keys: Vec<K> = keys.into_iter().collect();
        let mut commands = Vec::with_capacity(keys.len());
        for key in &keys {
            commands.push((key.as_ref().to_vec(), plan_get(key.as_ref(), None)?));
        }
        let mut found = HashMap::with_capacity(keys.len());
        let mut first_error = None;
        for (key, result) in keys.into_iter().zip(self.meta.exchange_many(commands)) {
            let view = match result.and_then(read_view) {
                Ok(view) => view,
                Err(error) => {
                    if self.policy().absorbs(&error) {
                        self.policy()
                            .report(ErrorKind::Degraded, "get_many", key.as_ref(), &error);
                    } else if first_error.is_none() {
                        first_error = Some(error);
                    }
                    continue;
                }
            };
            if let Some(win) = view.stale_win() {
                self.return_win("get_many", key.as_ref(), win);
            }
            if let Some(value) = finish_get(view)? {
                found.insert(key, value);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(found),
        }
    }

    fn store(&self, key: &[u8], value: &Encoded, ttl: Ttl, mode: SetMode) -> Result<bool> {
        let command = plan_store(key, value, ttl, mode, None)?;
        finish_store(&self.exchange(key, &command)?)
    }

    /// Store a value for `ttl`.
    pub fn set(&self, key: impl AsRef<[u8]>, value: impl Encode, ttl: impl Into<Ttl>) -> Result<()> {
        let key = key.as_ref();
        let encoded = encode_value(value)?;
        let result = self.store(key, &encoded, ttl.into(), SetMode::Set).and_then(|stored| {
            if stored {
                Ok(())
            } else {
                Err(Error::protocol("unconditional set was not stored"))
            }
        });
        self.policy().settle("set", key, result, || ())
    }

    /// Store several values for `ttl`, one round trip per server. Every
    /// value is encoded before anything is written. Without `degrade` the
    /// first failing server group fails the call after every group ran,
    /// the successful groups are not rolled back; with it, failing groups
    /// are dropped silently.
    pub fn set_many<K, V>(&self, pairs: impl IntoIterator<Item = (K, V)>, ttl: impl Into<Ttl>) -> Result<()>
    where
        K: AsRef<[u8]>,
        V: Encode,
    {
        let ttl = ttl.into();
        let mut commands = Vec::new();
        for (key, value) in pairs {
            let encoded = encode_value(value)?;
            commands.push((
                key.as_ref().to_vec(),
                plan_store(key.as_ref(), &encoded, ttl, SetMode::Set, None)?,
            ));
        }
        self.finish_writes("set_many", commands, |wire| {
            finish_store(wire).and_then(|stored| {
                if stored {
                    Ok(())
                } else {
                    Err(Error::protocol("unconditional set was not stored"))
                }
            })
        })
    }

    fn finish_writes(
        &self,
        op: &'static str,
        commands: Vec<(Vec<u8>, MetaCommand)>,
        finish: impl Fn(&MetaCommandResult) -> Result<()>,
    ) -> Result<()> {
        let keys: Vec<Vec<u8>> = commands.iter().map(|(key, _)| key.clone()).collect();
        let mut first_error = None;
        for (key, result) in keys.iter().zip(self.meta.exchange_many(commands)) {
            if let Err(error) = result.and_then(|wire| finish(&wire)) {
                if self.policy().absorbs(&error) {
                    self.policy().report(ErrorKind::Degraded, op, key, &error);
                } else if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Store only if the key is absent; `true` when this call won. Never
    /// degrades: the answer is the point.
    #[must_use = "add reports whether this call won the key"]
    pub fn add(&self, key: impl AsRef<[u8]>, value: impl Encode, ttl: impl Into<Ttl>) -> Result<bool> {
        let encoded = encode_value(value)?;
        self.store(key.as_ref(), &encoded, ttl.into(), SetMode::Add)
    }

    /// Store only if the key exists; never resurrects a deleted key.
    #[must_use = "replace reports whether the key still existed"]
    pub fn replace(&self, key: impl AsRef<[u8]>, value: impl Encode, ttl: impl Into<Ttl>) -> Result<bool> {
        let encoded = encode_value(value)?;
        self.store(key.as_ref(), &encoded, ttl.into(), SetMode::Replace)
    }

    /// Delete a key. A missing key is not an error.
    pub fn delete(&self, key: impl AsRef<[u8]>) -> Result<()> {
        let key = key.as_ref();
        let result = plan_erase(key, None, None)
            .and_then(|command| self.exchange(key, &command))
            .and_then(|wire| finish_erase(&wire).map(|_| ()));
        self.policy().settle("delete", key, result, || ())
    }

    /// Delete several keys, one round trip per server; failure semantics
    /// as [`set_many`](Self::set_many).
    pub fn delete_many<K: AsRef<[u8]>>(&self, keys: impl IntoIterator<Item = K>) -> Result<()> {
        let mut commands = Vec::new();
        for key in keys {
            commands.push((key.as_ref().to_vec(), plan_erase(key.as_ref(), None, None)?));
        }
        self.finish_writes("delete_many", commands, |wire| finish_erase(wire).map(|_| ()))
    }

    /// Soft delete: mark the item stale for `grace`. Plain reads keep
    /// serving the old value; `fetch` readers elect one recomputation and
    /// the rest keep the old value until it lands. `update` and `take`
    /// treat a stale item as a miss. A missing key is not an error.
    pub fn invalidate(&self, key: impl AsRef<[u8]>, grace: Duration) -> Result<()> {
        let key = key.as_ref();
        let result = grace_ttl(grace)
            .and_then(|grace| plan_erase(key, Some(grace), None))
            .and_then(|command| self.exchange(key, &command))
            .and_then(|wire| finish_erase(&wire).map(|_| ()));
        self.policy().settle("invalidate", key, result, || ())
    }

    /// Extend a key's expiry without transferring the value. Blind: it
    /// also extends an item kept stale by [`invalidate`](Self::invalidate).
    /// A missing key is not an error.
    pub fn touch(&self, key: impl AsRef<[u8]>, ttl: impl Into<Ttl>) -> Result<()> {
        let key = key.as_ref();
        let result = plan_touch(key, ttl.into()).and_then(|command| self.read("touch", key, &command).map(|_| ()));
        self.policy().settle("touch", key, result, || ())
    }

    /// Item metadata without transferring the value or bumping the LRU;
    /// `Ok(None)` when the key is not cached. A diagnostic probe, not
    /// something to branch business logic on.
    pub fn inspect(&self, key: impl AsRef<[u8]>) -> Result<Option<ItemInfo>> {
        let key = key.as_ref();
        let result = plan_inspect(key).and_then(|command| self.read("inspect", key, &command));
        self.policy()
            .settle("inspect", key, result, ReadView::default)
            .map(|view| finish_inspect(&view))
    }

    fn counter(&self, key: &[u8], delta: u64, decrement: bool, ttl: Ttl) -> Result<u64> {
        let command = plan_counter(key, delta, decrement, ttl)?;
        finish_counter(&self.exchange(key, &command)?)
    }

    /// Add `delta` to a counter; a miss counts from zero. `ttl` applies
    /// only when this call creates the counter, later increments never
    /// extend it (a fixed window). Never degrades.
    pub fn incr(&self, key: impl AsRef<[u8]>, delta: u64, ttl: impl Into<Ttl>) -> Result<u64> {
        self.counter(key.as_ref(), delta, false, ttl.into())
    }

    /// Subtract `delta` from a counter, saturating at zero; `ttl` applies
    /// at creation only, see [`incr`](Self::incr).
    pub fn decr(&self, key: impl AsRef<[u8]>, delta: u64, ttl: impl Into<Ttl>) -> Result<u64> {
        self.counter(key.as_ref(), delta, true, ttl.into())
    }

    fn concat(&self, op: &'static str, key: &[u8], fragment: &[u8], ttl: Ttl, prepend: bool) -> Result<()> {
        let result = plan_concat(key, fragment, ttl, prepend)
            .and_then(|command| self.exchange(key, &command))
            .and_then(|wire| finish_concat(&wire));
        self.policy().settle(op, key, result, || ())
    }

    /// Append raw bytes, creating the item on a miss. `ttl` applies only
    /// when this call creates the item. Bytes bypass [`Encode`]; read the
    /// buffer back as `Vec<u8>` (usually with [`take`](Self::take)).
    pub fn append(&self, key: impl AsRef<[u8]>, fragment: &[u8], ttl: impl Into<Ttl>) -> Result<()> {
        self.concat("append", key.as_ref(), fragment, ttl.into(), false)
    }

    /// Prepend raw bytes; see [`append`](Self::append).
    pub fn prepend(&self, key: impl AsRef<[u8]>, fragment: &[u8], ttl: impl Into<Ttl>) -> Result<()> {
        self.concat("prepend", key.as_ref(), fragment, ttl.into(), true)
    }

    /// Atomically transform a value: read with version, apply `f`, write
    /// back only if unchanged, retry on conflict. `f` receives `None` on a
    /// miss (or an item kept stale by [`invalidate`](Self::invalidate)),
    /// may run several times and must be pure. Exhausted retries fail with
    /// [`Error::Conflict`]. Never degrades.
    pub fn update<T, F>(&self, key: impl AsRef<[u8]>, ttl: impl Into<Ttl>, mut f: F) -> Result<T>
    where
        T: Encode + Decode,
        F: FnMut(Option<T>) -> T,
    {
        self.try_update(key, ttl, |current| Ok::<T, std::convert::Infallible>(f(current)))
    }

    /// [`update`](Self::update) with a fallible transform: an `Err` from
    /// `f` aborts the call without writing and surfaces as
    /// [`Error::Callback`].
    pub fn try_update<T, E, F>(&self, key: impl AsRef<[u8]>, ttl: impl Into<Ttl>, mut f: F) -> Result<T>
    where
        T: Encode + Decode,
        E: Into<BoxError>,
        F: FnMut(Option<T>) -> std::result::Result<T, E>,
    {
        let key = key.as_ref();
        let ttl = ttl.into();
        let probe = plan_probe(key)?;
        for _ in 0..UPDATE_ATTEMPTS {
            let step = update_step(read_view(self.exchange(key, &probe)?)?);
            let current = step.current.map(|(bytes, flags)| T::decode(bytes, flags)).transpose()?;
            let next = match f(current) {
                Ok(next) => next,
                Err(error) => {
                    // Nothing gets written; a stale token won by the probe
                    // must go back so fetch can still elect.
                    if let Some(win) = step.stale_win {
                        self.return_win("update", key, win);
                    }
                    return Err(callback_error(error));
                }
            };
            let encoded = match encode_value(&next) {
                Ok(encoded) => encoded,
                Err(error) => {
                    if let Some(win) = step.stale_win {
                        self.return_win("update", key, win);
                    }
                    return Err(error);
                }
            };
            // Any existing item (stale or a placeholder included) can only
            // be replaced through its CAS; only a true miss is an add.
            let command = match step.compare_cas {
                Some(cas) => plan_store(key, &encoded, ttl, SetMode::Set, Some(cas))?,
                None => plan_store(key, &encoded, ttl, SetMode::Add, None)?,
            };
            if finish_store(&self.exchange(key, &command)?)? {
                return Ok(next);
            }
        }
        Err(Error::Conflict)
    }

    /// Atomically read a value and delete it; `Ok(None)` when there was
    /// nothing to take (a stale item counts as nothing). The read and the
    /// delete are fenced by the item's version, so bytes appended in
    /// between are never lost. Never degrades.
    pub fn take<T: Decode>(&self, key: impl AsRef<[u8]>) -> Result<Option<T>> {
        let key = key.as_ref();
        let probe = plan_probe(key)?;
        for _ in 0..UPDATE_ATTEMPTS {
            match take_step(read_view(self.exchange(key, &probe)?)?)? {
                TakeStep::Nothing(win) => {
                    if let Some(win) = win {
                        self.return_win("take", key, win);
                    }
                    return Ok(None);
                }
                TakeStep::Take { value, flags, cas } => {
                    // Decode before deleting: a value that cannot be
                    // decoded stays on the server.
                    let value = T::decode(value, flags)?;
                    let command = plan_erase(key, None, Some(cas))?;
                    if finish_erase(&self.exchange(key, &command)?)? {
                        return Ok(Some(value));
                    }
                }
            }
        }
        Err(Error::Conflict)
    }

    /// Read a value or compute it exactly once. A hit returns the value. A
    /// miss takes a server-side lease: one caller per key runs `loader`
    /// and writes the result back for the `freshness` ttl, same-process
    /// callers share its result, other processes wait briefly and then
    /// compute locally without writing back. Inside a
    /// [`refresh_ahead`](Ttl::refresh_ahead) window or an
    /// [`invalidate`](Self::invalidate) grace period one reader is elected
    /// to recompute synchronously while the rest keep the current value.
    ///
    /// `fetch` never fails because coordination failed: every path ends in
    /// a value or the loader's own error ([`Error::Callback`]). Write-back
    /// failures only reach `on_error`. There is no overall time limit: a
    /// single exchange has the engine timeout, the loader has none.
    pub fn fetch<T, E, F>(&self, key: impl AsRef<[u8]>, freshness: impl Into<Freshness>, loader: F) -> Result<T>
    where
        T: Encode + Decode,
        E: Into<BoxError>,
        F: FnOnce() -> std::result::Result<T, E>,
    {
        let key = key.as_ref();
        let freshness = freshness.into();
        let ttl = freshness.ttl();
        let (command, _) = plan_election(key, freshness)?;
        let mut loader = Some(loader);
        for attempt in 0..=WAIT_BACKOFF.len() {
            let mut loader_now = || loader.take().expect("loader consumed once");
            let view = match self.exchange(key, &command).and_then(read_view) {
                Ok(view) => view,
                Err(error) => {
                    if self.policy().absorbs(&error) {
                        // Cache outage is not a site outage: compute
                        // locally, skip the cache, keep it observable.
                        self.policy().report(ErrorKind::Degraded, "fetch", key, &error);
                        return self.local_compute(key, loader_now());
                    }
                    return Err(error);
                }
            };
            match fetch_step(view) {
                FetchStep::Serve { value, flags } => return Ok(T::decode(value, flags)?),
                // The synchronous winner recomputes now; who pays how
                // much latency stays predictable, this client owns no
                // threads.
                FetchStep::Refresh { cas, .. } => return self.lead(key, ttl, cas, false, loader_now()),
                FetchStep::Lead { cas } => return self.lead(key, ttl, cas, true, loader_now()),
                FetchStep::Local => return self.local_compute(key, loader_now()),
                FetchStep::Wait => {
                    // Same-process callers share the leader's pending
                    // result; cross-process losers wait briefly and re-read.
                    if let Some(flight) = self.shared.existing(key) {
                        return decode_shared(flight.wait()?);
                    }
                    if attempt >= WAIT_BACKOFF.len() {
                        return self.local_compute(key, loader_now());
                    }
                    std::thread::sleep(WAIT_BACKOFF[attempt]);
                }
            }
        }
        unreachable!("fetch wait loop exits by return")
    }

    /// Run the loader as the elected winner, write back through `cas`,
    /// publish the encoded result to same-process waiters. The loader's
    /// error goes to everyone; write-back failures only to `on_error`.
    /// With `release`, a lease that cannot be repaid is given back so the
    /// next reader re-elects instead of waiting out the placeholder.
    fn lead<T, E, F>(&self, key: &[u8], ttl: Ttl, cas: u64, release: bool, loader: F) -> Result<T>
    where
        T: Encode + Decode,
        E: Into<BoxError>,
        F: FnOnce() -> std::result::Result<T, E>,
    {
        let (flight, leader) = self.shared.claim(key);
        if !leader {
            return decode_shared(flight.wait()?);
        }
        let lead = Lead {
            shared: &self.shared,
            key,
            flight,
            done: false,
        };
        let (value, encoded) = match loader().map_err(callback_error).and_then(|value| {
            let encoded = encode_value(&value)?;
            Ok((value, encoded))
        }) {
            Ok(ok) => ok,
            Err(error) => {
                lead.finish(Err(error.duplicate()));
                if release {
                    self.release_lease(key, cas);
                }
                if matches!(error, Error::EmptyValue | Error::Encode(_)) {
                    self.policy().report(ErrorKind::WriteBack, "fetch", key, &error);
                }
                return Err(error);
            }
        };
        self.write_back(key, &encoded, ttl, cas);
        lead.finish(Ok(Arc::new(encoded)));
        Ok(value)
    }

    /// Run the loader without write-back, still merged per process.
    fn local_compute<T, E, F>(&self, key: &[u8], loader: F) -> Result<T>
    where
        T: Encode + Decode,
        E: Into<BoxError>,
        F: FnOnce() -> std::result::Result<T, E>,
    {
        let (flight, leader) = self.shared.claim(key);
        if !leader {
            return decode_shared(flight.wait()?);
        }
        let lead = Lead {
            shared: &self.shared,
            key,
            flight,
            done: false,
        };
        match loader().map_err(callback_error).and_then(|value| {
            let encoded = encode_value(&value)?;
            Ok((value, encoded))
        }) {
            Ok((value, encoded)) => {
                lead.finish(Ok(Arc::new(encoded)));
                Ok(value)
            }
            Err(error) => {
                lead.finish(Err(error.duplicate()));
                Err(error)
            }
        }
    }

    /// Store a recomputed value conditioned on the election's CAS: a
    /// delete, set or re-invalidation that landed meanwhile wins, and the
    /// write is abandoned rather than resurrecting dead data.
    fn write_back(&self, key: &[u8], value: &Encoded, ttl: Ttl, cas: u64) {
        let outcome = plan_write_back(key, value, ttl, cas)
            .and_then(|command| self.exchange(key, &command))
            .and_then(|wire| finish_store(&wire));
        let error = match outcome {
            Ok(true) => return,
            Ok(false) => Error::protocol("fetch write-back abandoned: the item changed during recomputation"),
            Err(error) => error,
        };
        self.policy().report(ErrorKind::WriteBack, "fetch", key, &error);
    }

    fn release_lease(&self, key: &[u8], cas: u64) {
        let outcome = plan_release_lease(key, cas).and_then(|command| self.exchange(key, &command));
        if let Err(error) = outcome {
            self.policy().report(ErrorKind::LeaseReturn, "fetch", key, &error);
        }
    }
}
