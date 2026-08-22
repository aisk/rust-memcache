# 基于 meta 协议的高层客户端指南（实验性）

[English](https://github.com/aisk/rust-memcache/blob/master/docs/exp.md) | 简体中文

> **实验性。** 高层客户端位于 `memcache::exp` 下，其 API 可能在任意次版本中变化。
> 如果你依赖它，请在 `Cargo.toml` 中锁定**次版本号**。补丁版本（`x.y.Z`）不会引入破坏性变更，
> 但次版本（`x.Y.0`）可能会。
>
> ```toml
> [dependencies]
> memcache = "0.21"   # 允许 0.21.x，排除 0.22+
> ```

`memcache::exp::Memcache` 把 [meta 协议](https://github.com/memcached/memcached/blob/master/doc/protocol.txt) 藏在一组按用途命名的动词后面。协议里的 CAS 令牌和 lease 从不出现在调用方代码中。你不需要先读出版本号再写回去，而是带一个变换闭包调用 `update`，客户端内部完成读取、比较交换、重试的循环。你也不需要自己实现 dogpile 保护，带一个 loader 调用 `fetch`，客户端保证值只被计算一次。真正需要原始协议时，每条 meta 命令仍可通过 `cache.meta()` 访问。

## 创建客户端

```rust ignore
Memcache::builder()
    .timeout(Duration::from_secs(1))          // 每次交互的截止时间，None 表示不限
    .connect_timeout(Duration::from_secs(1))  // 拨号到服务器的时间上限
    .max_idle(8)                              // 每台服务器保留的空闲连接数
    .max_idle_age(Duration::from_secs(90))    // 空闲超过此时长的连接不再复用
    .max_connections(None)                    // 每台服务器正在使用的连接数上限
    .degrade(false)                           // 失败策略，见下文
    .on_error(|event| { /* 可观测性钩子 */ })
    .router(Rendezvous::default())            // 或用 hash_function(..) 在 rendezvous 下换一个哈希
    .connect(["cache1:11211", "cache2:11211"])?;
```

```rust ignore
use memcache::exp::Memcache;

let cache = Memcache::connect(["cache1:11211", "cache2:11211"])?;
```

多台服务器时，键通过 rendezvous 哈希分布。`hash_function` 替换哈希函数，`router` 通过 `Router` trait 替换整个放置策略。每台服务器有一个弹性连接池：`max_idle` 限制的是保留的空闲连接数，不是活跃请求数，需要上限时用 `max_connections` 限制后者。地址在连接时解析，连接按需延迟建立。客户端可以廉价克隆，所有克隆共享同一组连接池。

键可以是任何 `AsRef<[u8]>`，所以 `&str`、`String`、`&[u8]` 和 `Vec<u8>` 指向的是同一个条目。

## 值

值是业务对象，编码方式通过 `Encode` 和 `Decode` trait 放在值类型上。字符串、字节切片、vector 和各整数类型内置支持；在 `serde_json` feature 下，`Json<T>` 可以包装任意 `serde` 类型；你也可以为自己的类型实现这两个 trait。请求的类型决定解码方式，所以每次读取都明确说明它期望什么：

```rust ignore
use memcache::exp::Json;

cache.set("user:1", Json(&user), Ttl::secs(600))?;
let user: Option<Json<User>> = cache.get("user:1")?;
```

编码结果为零字节的值会被拒绝并返回 `Error::EmptyValue`，因为 memcached 用零字节条目表示 lease 占位符，所有读取都会把这种条目折叠成 miss。

## 读取

```rust ignore
cache.get::<T>(key)?                 // -> Option<T>
cache.get_and_touch::<T>(key, ttl)?  // -> Option<T>，命中时顺延过期时间
cache.get_many::<T, _>(keys)?        // -> HashMap<K, T>，只包含命中的键
cache.inspect(key)?                  // -> Option<ItemInfo>
```

`get` 读取一个值。miss 是正常结果而不是错误：它返回 `Ok(None)`，`Err` 只用于基础设施故障。两者从不混用。

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

`get_many` 每台服务器一个往返读取一组键，返回命中的值，键就是调用方传入的那些键值。miss 通过键的缺失表达。

`get_and_touch` 让同一条协议命令顺带顺延命中条目的过期时间，这样一次读取就变成了会话续期的读取一半：

```rust ignore
let session: Option<Session> = cache.get_and_touch(format!("session:{sid}"), Ttl::secs(1800))?;
```

这个顺延是 memcached 原生的 touch，而且是盲目的：它会延长读到的任何条目，包括被 `invalidate` 保持为陈旧的值。必须生效的吊销要走 `delete`。

`inspect` 返回条目的元数据（剩余 ttl、大小、最后访问时间、是否被命中过），不传输值，也不改变它的 LRU 位置。它是调试用的可观测性工具；基于元数据做业务分支天然存在竞态，不是受支持的用法。

## 写入

```rust ignore
cache.set(key, value, ttl)?          // -> ()
cache.set_many(pairs, ttl)?          // -> ()
cache.add(key, value, ttl)?          // -> bool，本次调用胜出时为 true
cache.replace(key, value, ttl)?      // -> bool，从不复活已删除的键
cache.touch(key, ttl)?               // -> ()
cache.delete(key)?                   // -> ()
cache.invalidate(key, grace)?        // -> ()
cache.delete_many(keys)?             // -> ()
```

`set` 无条件存储一个值，有效期为 `ttl`。`set_many` 每台服务器一个往返存储一批值，它们共享同一个 ttl。

每个存储方法都要求传入 `Ttl`，没有客户端级别的默认值。`Ttl::secs(n)` 和 `Duration` 表示从现在起的时长，`Ttl::at(SystemTime)` 是绝对的过期时刻，`Ttl::NEVER` 表示永不过期，让这个选择在调用处一目了然。零时长不是"永不"，会作为 `Error::Usage` 被拒绝；超过 30 天的时长按协议要求以绝对时间戳发送。

`add` 只在键不存在时存储，并报告本次调用是否胜出，这是一种分布式抢占：

```rust ignore
if cache.add("job:daily", "1", Ttl::secs(86400))? {
    run_daily_report();
}
```

`replace` 只在键存在时存储。它从不复活在此期间被删除的键，因此是会话处理的写入一半：被吊销的会话会保持吊销状态，即使某个更早加载过它的请求稍后又把它写回来。

```rust ignore
cache.replace(format!("session:{sid}"), &session, Ttl::secs(1800))?;
```

`touch` 用一条盲目的协议命令延长键的 ttl，不传输值。它为大值（渲染好的页面、序列化的报表）而存在，这种情况下只为续期而把负载读回来是在浪费带宽；如果本来就要读取，用 `get_and_touch`。

`delete` 直接抹掉键，下一个读者会承受一次完整的 miss；键不存在不算错误。`invalidate` 则把值标记为陈旧并保留一段宽限期：

```rust ignore
cache.delete(format!("article:{aid}"))?;                               // 硬删除：旧数据不得再出现
cache.invalidate(format!("article:{aid}"), Duration::from_secs(60))?;  // 软失效：读者短暂保留旧副本
```

宽限期内普通读者继续拿到旧副本，而 `fetch` 会选出一个调用方去重新计算；之后这个键衰退为普通 miss。软失效和由 `fetch` 管理的键配合使用，而且宽限期的上限只在没有人续期该键时成立，因为 touch 会像对待其它过期时间一样顺延它。如果旧值一秒都不能再被提供，用硬删除。

## 取或计算

```rust ignore
cache.fetch(key, freshness, loader)?   // -> T
```

最高频的缓存模式浓缩为一个动词：`get` 在调用方这边配一个静态兜底，`fetch` 则带一个 loader 计算值并写回。`freshness` 是一个 `Ttl`，可以通过 `Ttl::refresh_ahead` 附带一个提前刷新窗口；只有 `fetch` 接受 `Freshness`，所以这个修饰不会泄漏到普通写入里，而错误的组合（给 `Ttl::NEVER` 加窗口、窗口比 ttl 还宽）会在调用处以 `Error::Usage` 报错，而不是被静默忽略。

```rust ignore
let report: Report = cache.fetch("report:q3", Ttl::secs(3600), || build_report())?;
```

miss 时，所有进程中只有一个调用方赢得服务端 lease 并运行 loader。同一进程内的其它调用方等待这个结果，其它进程的调用方短暂等待后在本地计算且不写回。所以一个热点键在一千个并发请求下过期，代价是一次重新计算而不是一千次。

使用 `refresh_ahead` 时，剩余 ttl 进入窗口的值会照常返回，同时选出一个调用方重新计算，曲线上不会出现过期尖峰：

```rust ignore
let feed: Feed = cache.fetch(
    "home:feed",
    Ttl::secs(300).refresh_ahead(Duration::from_secs(30)),
    || build_feed(),
)?;
```

同步客户端中被选中的调用方就地重新计算，承担一次重算延迟（库不拥有任何线程，所以谁承担什么是可预测的）。异步客户端中被选中的调用方立即返回当前值，在客户端拥有的后台任务上重新计算，没有任何请求承担刷新延迟。

每次写回都以选举时观察到的版本为条件，所以重算过程中被删除的键不会被复活。写回失败不会改变 `fetch` 的返回值，它们会送到 `on_error` 钩子。`fetch` 不会因为协调失败而失败：每条路径最终要么得到一个值，要么得到 loader 自己的错误，后者以 `Error::Callback` 形式带出，可通过 `Error::callback::<E>()` 取回。

## 原子修改

```rust ignore
cache.update(key, ttl, |current| ..)?       // -> 新值
cache.try_update(key, ttl, |current| ..)?   // -> 新值，闭包可以失败
```

`update` 原子地变换一个值。它连同版本读出当前值，应用闭包，只在期间没有变化时写回，冲突时重试。闭包收到 `Option<T>`，miss 时为 `None`，所以起始值由它决定。闭包可能运行多次，因此必须是纯函数。`try_update` 接受可失败的闭包：返回 `Err` 会中止调用，条目保持未写入，错误以 `Error::Callback` 形式浮出。如果重试循环一直输给并发写入者，调用以 `Error::Conflict` 失败。被 `invalidate` 保持为陈旧的值算作 miss，因为对失效数据做变换会把它悄悄洗白成新鲜数据。

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

`incr` 给计数器加上 `delta` 并返回新值，miss 时创建计数器，所以第一个请求计为 `delta`。`decr` 做减法并在零处饱和。由于 ttl 在创建时固定，之后的调用从不延长它，这正好就是固定窗口限流：

```rust ignore
if cache.incr(format!("rate:{ip}"), 1, Ttl::secs(60))? > 100 {
    return Err(TooManyRequests);
}
```

```rust ignore
cache.append(key, fragment, ttl)?   // -> ()
cache.prepend(key, fragment, ttl)?  // -> ()
cache.take::<T>(key)?               // -> Option<T>，原子地取出并删除
```

`append` 和 `prepend` 把原始字节拼接到值上，miss 时创建；ttl 只作用于这次创建，之后的调用从不延长这个缓冲区的寿命。它们绕过 `Encode`，因为这类键的值模型是带分隔符的字节流而不是对象。`take` 原子地读取并删除一个值，不存在并发追加的字节被丢失的窗口。两者合起来构成"收集然后排空"的模式，比如按用户缓冲事件并定期取走整批：

```rust ignore
cache.append(format!("events:{uid}"), b"login;", Ttl::secs(86400))?;
let buffered: Option<Vec<u8>> = cache.take(format!("events:{uid}"))?;   // 由调用方拆分
```

`take` 不限于字节流；取走一个用 `set` 存入的一次性令牌，用法完全一样。

## 批量操作

```rust ignore
let found: HashMap<&str, Page> = cache.get_many(["a", "b", "c"])?;
cache.set_many([("a", &page_a), ("b", &page_b)], Ttl::secs(300))?;
cache.delete_many(["a", "b", "c"])?;
```

`_many` 系列动词按服务器分组键，每台服务器一个往返。所有值在写入任何东西之前先完成编码，所以编码错误不会碰到缓存。不启用 `degrade` 时，每个服务器分组都会执行，第一个失败的分组让整个调用失败；已成功的分组不会回滚。启用 `degrade` 时，失败的服务器只会从 `get_many` 结果中移除它自己的键，它的写入被静默丢弃。

不同键上的独立操作没有高层管线：同步客户端按顺序发出，异步客户端用 `tokio::join!` 并发执行，需要混合命令在一个往返里完成时，下沉到协议层的 `run_batch`。

## 失败策略

默认情况下每个基础设施故障都以 `Error` 浮出（`Io`、`Timeout`、`Protocol`、`Server` 等）。构造时的 `degrade(true)` 策略把缓存故障和站点故障解耦：

```rust ignore
let cache = Memcache::builder()
    .degrade(true)
    .on_error(|event| metrics.count(event.op, &event.kind))
    .connect(servers)?;
```

降级模式下，读取把失败报告为 miss，`fetch` 在本地计算且不写回，盲写（`set`、`delete`、`invalidate`、`touch`、`append` 等）静默放弃。答案会进入业务决策的动词（`add`、`replace`、`incr`、`decr`、`update`、`take`）即使在降级模式下也仍然大声失败，因为编造一个答案比失败更糟。`Error::Ambiguous`（请求已写出但结果未知，写入可能已经落地）始终浮出：降级覆盖的是"缓存挂了"，从不覆盖"写入可能成功也可能没有"。每个被吸收的失败仍会以 `ErrorEvent` 的形式到达 `on_error` 钩子，携带动词、吸收类型和错误本身，所以降级业务行为从不降级可观测性。客户端从不在写入开始后自动重试命令，因为盲目重试算术或 append 可能把修改应用两次；`Error::is_retryable` 告诉你请求什么时候确定没有被应用。

## 异步客户端

`tokio` feature 下的 `AsyncMemcache` 是同一套动词加上 `.await`；`fetch` 接受 future 而不是闭包，`try_update` 接受 async 闭包。

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

loader 运行在客户端拥有的独立任务上：miss 时它的结果被同一进程内的所有调用方共享，并且在其中任何一个被取消时仍然存活；在提前刷新窗口内或 `invalidate` 宽限期内，当前值立即返回，loader 在后台重新计算。命中时 loader 会被丢弃而不被 poll。每个动词都返回 `Send` 的 future，所以可以跨 `tokio::spawn` 和 `try_join!` 持有。

## 协议访问

高层动词没有覆盖的一切都在 `cache.meta()`（或独立的 `MetaClient` / `AsyncMetaClient`）后面，它是 meta 协议的 1:1 类型化映射：`get` / `set` / `delete` / `increment` / `decrement`（`mg` / `ms` / `md` / `ma`），每个协议标志对应一个 builder 方法。它操作原始字节加客户端 flags，返回轻度解析的结果，以值的形式报告 miss、CAS 不匹配和 lease 状态，不做编码或语义映射。

```rust ignore
use memcache::exp::{Get, Meta, Set, Ttl};

let client = cache.meta();
let stored = client.set("key", b"payload").ttl(Ttl::secs(60)).return_cas().send()?;
let got = client.get("key").meta(Meta::NONE.cas().ttl()).send()?;
assert_eq!(got.item.cas, stored.cas);

// 多个操作在每台服务器一个往返内完成，每个操作一个结果。
let results = client.run_batch(vec![
    Set::new("a", "1").ttl(Ttl::secs(60)).into(),
    Get::new("b").into(),
])?;

// lease、CAS 和 stale 标志都在，可以手工编排流程。
let read = client.get("hot").lease_ttl(30).refresh_before(10).send()?;
if read.won_lease() {
    client.set("hot", recompute()).ttl(Ttl::secs(300)).compare_cas(read.item.cas.unwrap()).send()?;
}
```

再往下，`MetaCommand` / `MetaResponse` 以及 `build_*` / `parse_*` 函数暴露了线路层，用于类型化操作没有覆盖的任何场景。完整的动词表和协议层参考见 `memcache::exp` 模块文档。
