//! The tokio high-level client: the verb table of [`Memcache`](super::Memcache)
//! with `.await`, and a `fetch` whose loader runs on a detached task.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use tokio::net::ToSocketAddrs;
use tokio::sync::watch;

use super::async_client::AsyncMetaClient;
use super::core::verbs::{
    FetchStep, ItemInfo, LEASE_TTL_SECS, ReadView, StaleWin, TakeStep, Token, UPDATE_ATTEMPTS, WAIT_BACKOFF,
    encode_value, fetch_step, finish_concat, finish_counter, finish_erase, finish_get, finish_inspect, finish_set,
    finish_store, grace_ttl, plan_concat, plan_counter, plan_election, plan_erase, plan_get, plan_give_back,
    plan_inspect, plan_probe, plan_store, plan_touch, plan_write_back, read_view, take_step, update_step,
};
use super::error::{Error, Result};
use super::memcache::{BoxError, ErrorKind, MemcacheBuilder, Policy, callback_error, decode_shared};
use super::meta_api::{MetaCommandResult, SetMode};
use super::meta_command::MetaCommand;
use super::router::ServerAddress;
use super::ttl::{Freshness, Ttl};
use super::value::{Decode, Encode, Encoded};

impl MemcacheBuilder {
    /// Connect to one or more servers; the async counterpart of
    /// [`connect`](Self::connect). Must run inside a tokio runtime, which
    /// `fetch`'s background tasks also need.
    pub async fn connect_async<A: ToSocketAddrs + ServerAddress>(
        self,
        addrs: impl IntoIterator<Item = A>,
    ) -> Result<AsyncMemcache> {
        let (shutdown, _) = watch::channel(());
        Ok(AsyncMemcache {
            meta: self.meta.connect_multiple_async(addrs).await?,
            shared: Arc::new(Shared {
                policy: self.policy,
                flights: Mutex::new(HashMap::new()),
                refreshing: Arc::new(Mutex::new(HashSet::new())),
                shutdown,
            }),
        })
    }
}

struct Shared {
    policy: Policy,
    /// In-process singleflight: the receiver resolves to the leader's
    /// encoded result; the sender lives on the leader's detached task.
    flights: Mutex<HashMap<Vec<u8>, watch::Receiver<FlightState>>>,
    /// Keys with a background refresh in progress; at most one per key.
    refreshing: Arc<Mutex<HashSet<Vec<u8>>>>,
    /// Dropped with the last client clone; background tasks watch it and
    /// stop when it closes.
    shutdown: watch::Sender<()>,
}

enum FlightState {
    Pending,
    Done(Result<Arc<Encoded>>),
}

#[derive(Debug)]
struct LoaderAborted;

impl fmt::Display for LoaderAborted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fetch loader task panicked or was cancelled")
    }
}

impl std::error::Error for LoaderAborted {}

async fn wait_flight(mut receiver: watch::Receiver<FlightState>) -> Result<Arc<Encoded>> {
    loop {
        if let FlightState::Done(result) = &*receiver.borrow_and_update() {
            return match result {
                Ok(encoded) => Ok(Arc::clone(encoded)),
                Err(error) => Err(error.duplicate()),
            };
        }
        if receiver.changed().await.is_err() {
            // The leader's task went away without publishing.
            return Err(Error::Callback(Arc::new(LoaderAborted)));
        }
    }
}

/// Run `future` unless the client is dropped first; `None` on shutdown.
async fn until_shutdown<F: Future>(future: F, mut shutdown: watch::Receiver<()>) -> Option<F::Output> {
    let mut future = Box::pin(future);
    let mut closed = Box::pin(async move { shutdown.changed().await });
    std::future::poll_fn(|context| {
        if let Poll::Ready(output) = future.as_mut().poll(context) {
            return Poll::Ready(Some(output));
        }
        match Pin::new(&mut closed).poll(context) {
            // Only closure ends the wait; the channel never carries values.
            Poll::Ready(Err(_)) => Poll::Ready(None),
            _ => Poll::Pending,
        }
    })
    .await
}

/// The tokio high-level memcached client built on the meta protocol: the verb table of
/// [`Memcache`](super::Memcache), every method `async`. See there for the
/// semantics shared by both clients; the differences are in `fetch`.
///
/// A `fetch` loader is a future that runs on a detached task: a miss-path
/// winner's result is shared by every same-process caller, so no single
/// caller's cancellation may abort it, and a refresh (inside a
/// [`refresh_ahead`](Ttl::refresh_ahead) window or an
/// [`invalidate`](Self::invalidate) grace period) returns the current value
/// immediately and recomputes in the background. That is why the loader
/// must be `Send + 'static`. Background tasks stop when the last clone of
/// the client is dropped.
///
/// Every method returns a `Send` future.
///
/// ```no_run
/// use memcache::exp::{AsyncMemcache, Ttl};
///
/// # async fn example() -> memcache::exp::Result<()> {
/// let cache = AsyncMemcache::connect(["127.0.0.1:11211"]).await?;
/// cache.set("greeting", "hello", Ttl::secs(60)).await?;
/// let greeting: Option<String> = cache.get("greeting").await?;
/// let report: String = cache
///     .fetch("report", Ttl::secs(300), async { Ok::<_, std::io::Error>(String::from("computed")) })
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct AsyncMemcache {
    meta: AsyncMetaClient,
    shared: Arc<Shared>,
}

impl fmt::Debug for AsyncMemcache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncMemcache")
            .field("degrade", &self.shared.policy.degrade)
            .finish_non_exhaustive()
    }
}

impl AsyncMemcache {
    /// Connect with the default configuration; see [`builder`](Self::builder).
    pub async fn connect<A: ToSocketAddrs + ServerAddress>(
        addrs: impl IntoIterator<Item = A>,
    ) -> Result<AsyncMemcache> {
        MemcacheBuilder::new().connect_async(addrs).await
    }

    pub fn builder() -> MemcacheBuilder {
        MemcacheBuilder::new()
    }

    /// The protocol layer: every meta command with every option.
    pub fn meta(&self) -> &AsyncMetaClient {
        &self.meta
    }

    fn policy(&self) -> &Policy {
        &self.shared.policy
    }

    async fn exchange(&self, key: &[u8], command: &MetaCommand) -> Result<MetaCommandResult> {
        self.meta.exchange(key, command).await
    }

    /// Hand an accidentally won stale-recache token back on a detached
    /// task, so the read that won it does not pay the round trip.
    fn return_win(&self, op: &'static str, key: &[u8], win: StaleWin) {
        self.spawn_give_back(op, key, Token::Win(win));
    }

    /// Give a fetch token back without waiting for the round trip.
    fn give_back(&self, key: &[u8], token: Token) {
        self.spawn_give_back("fetch", key, token);
    }

    fn spawn_give_back(&self, op: &'static str, key: &[u8], token: Token) {
        let meta = self.meta.clone();
        let policy = self.policy().clone();
        let key = key.to_vec();
        tokio::spawn(async move {
            let outcome = match plan_give_back(&key, token) {
                Ok(command) => meta.exchange(&key, &command).await.map(|_| ()),
                Err(error) => Err(error),
            };
            if let Err(error) = outcome {
                policy.report(ErrorKind::LeaseReturn, op, &key, &error);
            }
        });
    }

    async fn read(&self, op: &'static str, key: &[u8], command: &MetaCommand) -> Result<ReadView> {
        let view = read_view(self.exchange(key, command).await?)?;
        if let Some(win) = view.stale_win() {
            self.return_win(op, key, win);
        }
        Ok(view)
    }

    /// Read a value; `Ok(None)` on a miss.
    pub async fn get<T: Decode>(&self, key: impl AsRef<[u8]>) -> Result<Option<T>> {
        let key = key.as_ref();
        let result = match plan_get(key, None) {
            Ok(command) => self.read("get", key, &command).await,
            Err(error) => Err(error),
        };
        self.policy()
            .settle("get", key, result, ReadView::default)
            .and_then(finish_get)
    }

    /// Read a value and slide its expiry; see [`Memcache::get_and_touch`](super::Memcache::get_and_touch).
    pub async fn get_and_touch<T: Decode>(&self, key: impl AsRef<[u8]>, ttl: impl Into<Ttl>) -> Result<Option<T>> {
        let key = key.as_ref();
        let result = match plan_get(key, Some(ttl.into())) {
            Ok(command) => self.read("get_and_touch", key, &command).await,
            Err(error) => Err(error),
        };
        self.policy()
            .settle_read("get_and_touch", key, result, ReadView::default)
            .and_then(finish_get)
    }

    /// Read several keys, one round trip per server, the servers in
    /// parallel; see [`Memcache::get_many`](super::Memcache::get_many).
    pub async fn get_many<T, K>(&self, keys: impl IntoIterator<Item = K>) -> Result<HashMap<K, T>>
    where
        T: Decode,
        K: AsRef<[u8]> + Hash + Eq,
    {
        let keys: Vec<K> = keys.into_iter().collect();
        let mut commands = Vec::with_capacity(keys.len());
        for key in &keys {
            commands.push((key.as_ref().to_vec(), plan_get(key.as_ref(), None)?));
        }
        let results = self.meta.exchange_many(&commands).await;
        let mut found = HashMap::with_capacity(keys.len());
        let mut first_error = None;
        for (key, result) in keys.into_iter().zip(results) {
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

    async fn store(&self, key: &[u8], value: &Encoded, ttl: Ttl, mode: SetMode) -> Result<bool> {
        let command = plan_store(key, value, ttl, mode, None)?;
        finish_store(&self.exchange(key, &command).await?)
    }

    /// Store a value for `ttl`.
    pub async fn set(&self, key: impl AsRef<[u8]>, value: impl Encode, ttl: impl Into<Ttl>) -> Result<()> {
        let key = key.as_ref();
        let encoded = encode_value(value)?;
        let result = match plan_store(key, &encoded, ttl.into(), SetMode::Set, None) {
            Ok(command) => self.exchange(key, &command).await.and_then(|wire| finish_set(&wire)),
            Err(error) => Err(error),
        };
        self.policy().settle("set", key, result, || ())
    }

    /// Store several values; see [`Memcache::set_many`](super::Memcache::set_many).
    pub async fn set_many<K, V>(&self, pairs: impl IntoIterator<Item = (K, V)>, ttl: impl Into<Ttl>) -> Result<()>
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
        self.finish_writes("set_many", commands, finish_set).await
    }

    async fn finish_writes(
        &self,
        op: &'static str,
        commands: Vec<(Vec<u8>, MetaCommand)>,
        finish: impl Fn(&MetaCommandResult) -> Result<()>,
    ) -> Result<()> {
        let results = self.meta.exchange_many(&commands).await;
        let mut first_error = None;
        for ((key, _), result) in commands.iter().zip(results) {
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

    /// Store only if the key is absent; `true` when this call won.
    #[must_use = "add reports whether this call won the key"]
    pub async fn add(&self, key: impl AsRef<[u8]>, value: impl Encode, ttl: impl Into<Ttl>) -> Result<bool> {
        let encoded = encode_value(value)?;
        self.store(key.as_ref(), &encoded, ttl.into(), SetMode::Add).await
    }

    /// Store only if the key exists; never resurrects a deleted key.
    #[must_use = "replace reports whether the key still existed"]
    pub async fn replace(&self, key: impl AsRef<[u8]>, value: impl Encode, ttl: impl Into<Ttl>) -> Result<bool> {
        let encoded = encode_value(value)?;
        self.store(key.as_ref(), &encoded, ttl.into(), SetMode::Replace).await
    }

    async fn erase(&self, op: &'static str, key: &[u8], grace: Option<Ttl>) -> Result<()> {
        let result = match plan_erase(key, grace, None) {
            Ok(command) => self
                .exchange(key, &command)
                .await
                .and_then(|wire| finish_erase(&wire).map(|_| ())),
            Err(error) => Err(error),
        };
        self.policy().settle(op, key, result, || ())
    }

    /// Delete a key. A missing key is not an error.
    pub async fn delete(&self, key: impl AsRef<[u8]>) -> Result<()> {
        self.erase("delete", key.as_ref(), None).await
    }

    /// Delete several keys; see [`Memcache::delete_many`](super::Memcache::delete_many).
    pub async fn delete_many<K: AsRef<[u8]>>(&self, keys: impl IntoIterator<Item = K>) -> Result<()> {
        let mut commands = Vec::new();
        for key in keys {
            commands.push((key.as_ref().to_vec(), plan_erase(key.as_ref(), None, None)?));
        }
        self.finish_writes("delete_many", commands, |wire| finish_erase(wire).map(|_| ()))
            .await
    }

    /// Soft delete; see [`Memcache::invalidate`](super::Memcache::invalidate).
    pub async fn invalidate(&self, key: impl AsRef<[u8]>, grace: Duration) -> Result<()> {
        let key = key.as_ref();
        let grace = match grace_ttl(grace) {
            Ok(grace) => grace,
            Err(error) => return self.policy().settle("invalidate", key, Err(error), || ()),
        };
        self.erase("invalidate", key, Some(grace)).await
    }

    /// Extend a key's expiry blindly; see [`Memcache::touch`](super::Memcache::touch).
    pub async fn touch(&self, key: impl AsRef<[u8]>, ttl: impl Into<Ttl>) -> Result<()> {
        let key = key.as_ref();
        let result = match plan_touch(key, ttl.into()) {
            Ok(command) => self.read("touch", key, &command).await.map(|_| ()),
            Err(error) => Err(error),
        };
        self.policy().settle("touch", key, result, || ())
    }

    /// Item metadata; see [`Memcache::inspect`](super::Memcache::inspect).
    pub async fn inspect(&self, key: impl AsRef<[u8]>) -> Result<Option<ItemInfo>> {
        let key = key.as_ref();
        let result = match plan_inspect(key) {
            Ok(command) => self.read("inspect", key, &command).await,
            Err(error) => Err(error),
        };
        self.policy()
            .settle("inspect", key, result, ReadView::default)
            .map(|view| finish_inspect(&view))
    }

    async fn counter(&self, key: &[u8], delta: u64, decrement: bool, ttl: Ttl) -> Result<u64> {
        let command = plan_counter(key, delta, decrement, ttl)?;
        finish_counter(&self.exchange(key, &command).await?)
    }

    /// Add `delta` to a counter; `ttl` applies at creation only. See
    /// [`Memcache::incr`](super::Memcache::incr).
    pub async fn incr(&self, key: impl AsRef<[u8]>, delta: u64, ttl: impl Into<Ttl>) -> Result<u64> {
        self.counter(key.as_ref(), delta, false, ttl.into()).await
    }

    /// Subtract `delta`, saturating at zero; `ttl` applies at creation only.
    pub async fn decr(&self, key: impl AsRef<[u8]>, delta: u64, ttl: impl Into<Ttl>) -> Result<u64> {
        self.counter(key.as_ref(), delta, true, ttl.into()).await
    }

    async fn concat(&self, op: &'static str, key: &[u8], fragment: &[u8], ttl: Ttl, prepend: bool) -> Result<()> {
        let result = match plan_concat(key, fragment, ttl, prepend) {
            Ok(command) => self.exchange(key, &command).await.and_then(|wire| finish_concat(&wire)),
            Err(error) => Err(error),
        };
        self.policy().settle(op, key, result, || ())
    }

    /// Append raw bytes; `ttl` applies at creation only. See
    /// [`Memcache::append`](super::Memcache::append).
    pub async fn append(&self, key: impl AsRef<[u8]>, fragment: &[u8], ttl: impl Into<Ttl>) -> Result<()> {
        self.concat("append", key.as_ref(), fragment, ttl.into(), false).await
    }

    /// Prepend raw bytes; see [`append`](Self::append).
    pub async fn prepend(&self, key: impl AsRef<[u8]>, fragment: &[u8], ttl: impl Into<Ttl>) -> Result<()> {
        self.concat("prepend", key.as_ref(), fragment, ttl.into(), true).await
    }

    /// Atomically transform a value with a plain closure; see
    /// [`Memcache::update`](super::Memcache::update).
    pub async fn update<T, F>(&self, key: impl AsRef<[u8]>, ttl: impl Into<Ttl>, mut f: F) -> Result<T>
    where
        T: Encode + Decode,
        F: FnMut(Option<T>) -> T,
    {
        self.transform(key.as_ref(), ttl.into(), |current| {
            std::future::ready(Ok::<T, std::convert::Infallible>(f(current)))
        })
        .await
    }

    /// Atomically transform a value with a fallible async closure; see
    /// [`Memcache::try_update`](super::Memcache::try_update).
    pub async fn try_update<T, E, F, Fut>(&self, key: impl AsRef<[u8]>, ttl: impl Into<Ttl>, f: F) -> Result<T>
    where
        T: Encode + Decode,
        E: Into<BoxError>,
        F: FnMut(Option<T>) -> Fut,
        Fut: Future<Output = std::result::Result<T, E>> + Send,
    {
        self.transform(key.as_ref(), ttl.into(), f).await
    }

    /// The optimistic loop behind `update` and `try_update`. No `Send`
    /// bound here: the returned future is `Send` exactly when `T` and the
    /// closure's future are, which is what each public signature promises.
    async fn transform<T, E, F, Fut>(&self, key: &[u8], ttl: Ttl, mut f: F) -> Result<T>
    where
        T: Encode + Decode,
        E: Into<BoxError>,
        F: FnMut(Option<T>) -> Fut,
        Fut: Future<Output = std::result::Result<T, E>>,
    {
        let probe = plan_probe(key)?;
        for _ in 0..UPDATE_ATTEMPTS {
            let step = update_step(read_view(self.exchange(key, &probe).await?)?);
            let current = step.current.map(|(bytes, flags)| T::decode(bytes, flags)).transpose()?;
            let next = match f(current).await {
                Ok(next) => next,
                Err(error) => {
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
            let command = match step.compare_cas {
                Some(cas) => plan_store(key, &encoded, ttl, SetMode::Set, Some(cas))?,
                None => plan_store(key, &encoded, ttl, SetMode::Add, None)?,
            };
            if finish_store(&self.exchange(key, &command).await?)? {
                return Ok(next);
            }
        }
        Err(Error::Conflict)
    }

    /// Atomically read and delete; see [`Memcache::take`](super::Memcache::take).
    pub async fn take<T: Decode>(&self, key: impl AsRef<[u8]>) -> Result<Option<T>> {
        let key = key.as_ref();
        let probe = plan_probe(key)?;
        for _ in 0..UPDATE_ATTEMPTS {
            match take_step(read_view(self.exchange(key, &probe).await?)?)? {
                TakeStep::Nothing(win) => {
                    if let Some(win) = win {
                        self.return_win("take", key, win);
                    }
                    return Ok(None);
                }
                TakeStep::Take { value, flags, cas } => {
                    let value = T::decode(value, flags)?;
                    let command = plan_erase(key, None, Some(cas))?;
                    if finish_erase(&self.exchange(key, &command).await?)? {
                        return Ok(Some(value));
                    }
                }
            }
        }
        Err(Error::Conflict)
    }

    /// Read a value or compute it exactly once; see
    /// [`Memcache::fetch`](super::Memcache::fetch) for the shared
    /// semantics. Here `loader` is a future that runs on a detached task:
    /// on a miss its result is shared by every same-process caller and
    /// survives any one of them being cancelled; inside a refresh window
    /// or an invalidate grace period the current value is returned at once
    /// and the loader recomputes in the background, bounded by the window
    /// (or the 30 second lease on the grace path). A hit drops the loader
    /// unpolled.
    pub async fn fetch<T, E, Fut>(
        &self,
        key: impl AsRef<[u8]>,
        freshness: impl Into<Freshness>,
        loader: Fut,
    ) -> Result<T>
    where
        T: Encode + Decode + Send + 'static,
        E: Into<BoxError> + Send + 'static,
        Fut: Future<Output = std::result::Result<T, E>> + Send + 'static,
    {
        let key = key.as_ref();
        let freshness = freshness.into();
        let ttl = freshness.ttl();
        let (command, window) = plan_election(key, freshness)?;
        let mut loader = Some(loader);
        for attempt in 0..=WAIT_BACKOFF.len() {
            let mut loader_now = || loader.take().expect("loader consumed once");
            let view = match self.exchange(key, &command).await.and_then(read_view) {
                Ok(view) => view,
                Err(error) => {
                    if self.policy().absorbs_read(&error) {
                        self.policy().report(ErrorKind::Degraded, "fetch", key, &error);
                        return self.local_compute(key, loader_now()).await;
                    }
                    return Err(error);
                }
            };
            match fetch_step(view) {
                FetchStep::Serve { value, flags } => return Ok(T::decode(value, flags)?),
                FetchStep::Refresh { value, flags, win } => {
                    // Serve now, recompute in the background: blocking the
                    // winner would recreate the latency spike this path
                    // exists to remove.
                    let bound = Duration::from_secs(u64::from(window.unwrap_or(LEASE_TTL_SECS)));
                    self.spawn_refresh(key, ttl, win, bound, loader_now());
                    return Ok(T::decode(value, flags)?);
                }
                FetchStep::Lead { cas } => {
                    return self.lead(key, ttl, Some(Token::Lease { cas }), loader_now()).await;
                }
                FetchStep::Local => return self.local_compute(key, loader_now()).await,
                FetchStep::Wait => {
                    let existing = self.shared.flights.lock().unwrap().get(key).cloned();
                    if let Some(flight) = existing {
                        return decode_shared(wait_flight(flight).await?);
                    }
                    match WAIT_BACKOFF.get(attempt) {
                        Some(backoff) => tokio::time::sleep(*backoff).await,
                        None => return self.local_compute(key, loader_now()).await,
                    }
                }
            }
        }
        unreachable!("fetch wait loop exits by return")
    }

    /// Run the loader on a detached task as the flight leader (or join the
    /// existing flight). With a `token`, the result is written back through
    /// it; a token that is not repaid, because the loader failed or because
    /// another caller of this process already leads the key, is given back
    /// so the next reader re-elects instead of waiting it out.
    async fn lead<T, E, Fut>(&self, key: &[u8], ttl: Ttl, token: Option<Token>, loader: Fut) -> Result<T>
    where
        T: Encode + Decode + Send + 'static,
        E: Into<BoxError> + Send + 'static,
        Fut: Future<Output = std::result::Result<T, E>> + Send + 'static,
    {
        let receiver = {
            let mut flights = self.shared.flights.lock().unwrap();
            if let Some(existing) = flights.get(key) {
                if let Some(token) = token {
                    self.give_back(key, token);
                }
                existing.clone()
            } else {
                let (sender, receiver) = watch::channel(FlightState::Pending);
                flights.insert(key.to_vec(), receiver.clone());
                let meta = self.meta.clone();
                let policy = self.policy().clone();
                let shared = Arc::downgrade(&self.shared);
                let key = key.to_vec();
                tokio::spawn(async move {
                    let result = loader
                        .await
                        .map_err(callback_error)
                        .and_then(|value| encode_value(&value));
                    match (&result, token) {
                        (Ok(encoded), Some(token)) => write_back(&meta, &policy, &key, encoded, ttl, token.cas()).await,
                        (Err(error), Some(token)) => {
                            if matches!(error, Error::EmptyValue | Error::Encode(_)) {
                                policy.report(ErrorKind::WriteBack, "fetch", &key, error);
                            }
                            give_back(&meta, &policy, &key, token).await;
                        }
                        (_, None) => {}
                    }
                    if let Some(shared) = shared.upgrade() {
                        shared.flights.lock().unwrap().remove(&key);
                    }
                    // Waiters duplicate the error; the original stays here.
                    let _ = sender.send(FlightState::Done(result.map(Arc::new)));
                });
                receiver
            }
        };
        decode_shared(wait_flight(receiver).await?)
    }

    /// Run the loader without write-back, still merged per process.
    async fn local_compute<T, E, Fut>(&self, key: &[u8], loader: Fut) -> Result<T>
    where
        T: Encode + Decode + Send + 'static,
        E: Into<BoxError> + Send + 'static,
        Fut: Future<Output = std::result::Result<T, E>> + Send + 'static,
    {
        self.lead(key, Ttl::NEVER, None, loader).await
    }

    /// Recompute in the background after a refresh-ahead or stale-grace
    /// win, at most once per key at a time, bounded by the window that
    /// triggered it and by the client's lifetime. A win taken while a
    /// refresh is already running is handed back: the running refresh
    /// writes back through its own CAS, which this newer win has bumped
    /// past, so nobody would repay the token otherwise.
    fn spawn_refresh<T, E, Fut>(&self, key: &[u8], ttl: Ttl, win: StaleWin, bound: Duration, loader: Fut)
    where
        T: Encode + Send + 'static,
        E: Into<BoxError> + Send + 'static,
        Fut: Future<Output = std::result::Result<T, E>> + Send + 'static,
    {
        if !self.shared.refreshing.lock().unwrap().insert(key.to_vec()) {
            self.give_back(key, Token::Win(win));
            return;
        }
        let cas = win.cas;
        let meta = self.meta.clone();
        let policy = self.policy().clone();
        let refreshing = Arc::clone(&self.shared.refreshing);
        let shutdown = self.shared.shutdown.subscribe();
        let key = key.to_vec();
        tokio::spawn(async move {
            let loaded = until_shutdown(tokio::time::timeout(bound, loader), shutdown).await;
            match loaded {
                None => {}
                Some(Err(_)) => {
                    let error = Error::Callback(Arc::new(RefreshTimedOut(bound)));
                    policy.report(ErrorKind::BackgroundLoader, "fetch", &key, &error);
                }
                Some(Ok(Err(error))) => {
                    policy.report(ErrorKind::BackgroundLoader, "fetch", &key, &callback_error(error));
                }
                Some(Ok(Ok(value))) => match encode_value(&value) {
                    Ok(encoded) => write_back(&meta, &policy, &key, &encoded, ttl, cas).await,
                    Err(error) => policy.report(ErrorKind::WriteBack, "fetch", &key, &error),
                },
            }
            refreshing.lock().unwrap().remove(&key);
        });
    }
}

#[derive(Debug)]
struct RefreshTimedOut(Duration);

impl fmt::Display for RefreshTimedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "background refresh exceeded its {:?} window", self.0)
    }
}

impl std::error::Error for RefreshTimedOut {}

async fn write_back(meta: &AsyncMetaClient, policy: &Policy, key: &[u8], value: &Encoded, ttl: Ttl, cas: u64) {
    let outcome = match plan_write_back(key, value, ttl, cas) {
        Ok(command) => meta.exchange(key, &command).await.and_then(|wire| finish_store(&wire)),
        Err(error) => Err(error),
    };
    let error = match outcome {
        Ok(true) => return,
        Ok(false) => Error::protocol("fetch write-back abandoned: the item changed during recomputation"),
        Err(error) => error,
    };
    policy.report(ErrorKind::WriteBack, "fetch", key, &error);
}

async fn give_back(meta: &AsyncMetaClient, policy: &Policy, key: &[u8], token: Token) {
    let outcome = match plan_give_back(key, token) {
        Ok(command) => meta.exchange(key, &command).await.map(|_| ()),
        Err(error) => Err(error),
    };
    if let Err(error) = outcome {
        policy.report(ErrorKind::LeaseReturn, "fetch", key, &error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every verb must return a `Send` future, so callers can hold one
    /// across `tokio::spawn` and `try_join!`.
    #[allow(dead_code)]
    fn futures_are_send(cache: &AsyncMemcache) {
        fn assert_send<F: Future + Send>(_: F) {}
        assert_send(cache.get::<String>("k"));
        assert_send(cache.get_and_touch::<String>("k", Ttl::secs(1)));
        assert_send(cache.get_many::<String, _>(["k"]));
        assert_send(cache.set("k", "v", Ttl::secs(1)));
        assert_send(cache.set_many([("k", "v")], Ttl::secs(1)));
        assert_send(cache.add("k", "v", Ttl::secs(1)));
        assert_send(cache.replace("k", "v", Ttl::secs(1)));
        assert_send(cache.delete("k"));
        assert_send(cache.delete_many(["k"]));
        assert_send(cache.invalidate("k", Duration::from_secs(1)));
        assert_send(cache.touch("k", Ttl::secs(1)));
        assert_send(cache.inspect("k"));
        assert_send(cache.incr("k", 1, Ttl::secs(1)));
        assert_send(cache.decr("k", 1, Ttl::secs(1)));
        assert_send(cache.append("k", b"x", Ttl::secs(1)));
        assert_send(cache.prepend("k", b"x", Ttl::secs(1)));
        assert_send(cache.update::<u64, _>("k", Ttl::secs(1), |c| c.unwrap_or(0)));
        assert_send(cache.try_update::<u64, std::io::Error, _, _>(
            "k",
            Ttl::secs(1),
            |c| async move { Ok(c.unwrap_or(0)) },
        ));
        assert_send(cache.take::<String>("k"));
        assert_send(cache.fetch("k", Ttl::secs(1), async { Ok::<_, std::io::Error>(String::new()) }));
    }
}
