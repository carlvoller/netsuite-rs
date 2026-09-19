# netsuite-rs
An async lightweight, familiar driver for [Oracle NetSuite](https://www.netsuite.com/) written **natively** in Rust. This driver uses SuiteQL for reads and the REST Record API for writes. Only Token-Based Authentication is supported.

## Features
- [x] Run SuiteQL queries, with positional bind parameters
- [x] Stream results, transparently paginating past NetSuite's page size, with configurable concurrent page fetching and optional retry-with-backoff on concurrency-limit errors
- [x] Parse results into `Column`, `Row`, `Cell` primitives for easy Rust usage
- [x] Best-effort `describe()` of a query's columns and bind count, without fetching all its rows
- [x] Create / get / update / replace / delete / upsert records via the REST Record API
- [x] Token-Based Authentication (OAuth 1.0a, HMAC-SHA256, using consumer key/secret + token id/secret + account id)
- [x] Custom API hostname, for sandbox accounts or non-standard domains
- [x] `bigdecimal` parsing of numeric cell values (feature flag)
- [x] Lightweight by design with minimal dependencies

## Motivation
I needed a way to connect to Oracle NetSuite in Rust, but couldn't find any existing solution. Figured I would just make this cause why not 🤷‍♂️.

I'm currently actively using this in real production workloads, but thats NOT the same as saying this package has been battle tested. Use at your own risk.

## Installation
```bash
$ cargo add netsuite-rs
```

### Cargo Feature Flags
```toml
# Cargo.toml
netsuite-rs = { version = "1", features = ["decimal"] }
```

- `decimal`: Deserialize numeric cell values into a `bigdecimal::BigDecimal` instead of `f64`. Recommended if you're working with money or need exact precision.

## Usage

### Creating a NetSuite Connection

NetSuite Token-Based Authentication needs five things:
1. NetSuite Account ID
2. Consumer Key
3. Consumer Secret
4. Token ID
5. Token Secret

You can find these credentials under Setup/Integration Management and Setup/Users/Roles/Access Tokens in the Oracle NetSuite UI.

```rust
use netsuite_rs::NetsuiteConnectionOptsBuilder;

async fn main() {
    let opts = NetsuiteConnectionOptsBuilder::default()
        .account_id("1234567_SB1") // or "1234567-sb1" - either form works
        .consumer_key("CONSUMER_KEY")
        .consumer_secret("CONSUMER_SECRET")
        .token_id("TOKEN_ID")
        .token_secret("TOKEN_SECRET")
        .pool_size(5usize)                  // Optional, default 5
        .page_size(1000)                    // Optional, default 1000 (NetSuite's max)
        .page_concurrency(3usize)           // Optional, default 3
        .retry_on_concurrency_limit(false)  // Optional, default false
        .build()
        .unwrap();

    let pool = opts.connect().await.unwrap();
    let conn = pool.get().await.unwrap();

    // ...
}
```

The `connect()` method does makes a network call as NetSuite handles authentication by signing each request. `NetsuitePool::get()` just limits concurrency up to `pool_size`, capping how many requests are in flight at once.

### Queries

Run a `SELECT` query with the `Executor` trait:

```rust
use netsuite_rs::Executor;
use futures_util::TryStreamExt;

async fn main() {
    // ...
    let mut query = conn
        .query("SELECT id, entityid, companyname FROM customer WHERE id > ?")
        .await
        .unwrap();

    query.bind(100);

    let results = query.execute().await.unwrap();

    // rows is a BoxStream from futures_core. Pages are fetched transparently as you consume it.
    let mut rows = results.rows();

    while let Some(row) = rows.try_next().await.unwrap() {
        let cell = row.get_by_name("companyname").unwrap();
        println!("Column Name = {:?}", cell.col.name);
        println!("Value = {:?}", cell.value);
    }

    // Or just buffer everything into a Vec:
    let rows = conn.fetch_all("SELECT id FROM customer WHERE id > 100").await.unwrap();

    // ...
}
```

#### Pagination and `page_concurrency`

`.rows()` fetches subsequent pages under the hood using the connection's `page_size`. For a lot of queries a single page's cost is dominated by NetSuite's own query execution time rather than by row count or how deep into the result set you are.

> During my testing, there was identical per-page latency at offset 0 and at offset 90,000 with no change from adding/removing `ORDER BY` or a `WHERE` filter. When that's the bottleneck, fetching pages one at a time just wastes all the wait time instead of overlapping it.

`page_concurrency` (default `3`) controls how many page requests `.rows()` runs concurrently. It doesn't speed up any requests for individual pages but overlapping N of them turns `page_count * page_cost` sequential request time into roughly `(page_count / N) * page_cost`. I guarantee that rows are always yielded in page order regardless of which page's request actually completes first using `FuturesOrdered`.

> Unfortunately, NetSuite enforces a single concurrency limit shared account-wide across *all* REST, SOAP, and RESTlet traffic (not just this driver). This can be as low as 5 concurrent requests on an entry-tier account with no SuiteCloud Plus licenses. `3` is chosen to leave some headroom under that floor. You can check Setup > Integration > Integration Management > Integration Governance for your account's actual limit, and raise it if you have room, or drop it to `1` to go back to fully sequential fetching. Exceeding your account's limit surfaces as an error from whichever page request tripped it, same as any other failed page. (unless `retry_on_concurrency_limit` is configured)

> If a query's per-page execution time itself is the problem (as opposed to not overlapping requests), `page_concurrency` won't fix it. That's a query-shape/NetSuite-execution-cost issue out of this library's control, not a pagination one. Simplifying the query (fewer joins, fewer filter conditions, a simpler `ORDER BY`) is the only thing that reduces that cost.

#### `retry_on_concurrency_limit`

NetSuite's concurrency limit is one pool shared account-wide across *all* REST, SOAP, and RESTlet traffic. That means even a `page_concurrency` comfortably under your account's configured limit can occasionally collide with other activity in your account (like another integration, a scheduled script, someone in the UI) and get rejected with a `CONCURRENCY_LIMIT_EXCEEDED` error.

By default, I reject these requests outright and abort the whole stream. In order words, you lose everything already in progress on what might be a long-running fetch. Setting `.retry_on_concurrency_limit(true)` makes any failed request retry instead, with exponential backoff (500ms, 1s, 2s, 4s, 8s, up to 5 attempts) rather than failing the whole operation. I left it as opt-in because it changes error and timing behavior. (Also maybe you would want to know when you're hitting the concurrency limit)

Bind multiple parameters at once with the `row!` macro:

```rust
use netsuite_rs::row;

async fn main() {
    // ...
    let mut query = conn
        .query("SELECT id FROM customer WHERE id > ? AND id < ?")
        .await
        .unwrap();

    query.bind_row(row![100, 200]);

    let results = query.execute().await.unwrap();
    // ...
}
```

Describe a query to get its inferred column names/types and its parameter count, without fetching every row:

```rust
async fn main() {
    // ...
    let describe = conn
        .query("SELECT * FROM customer WHERE id > ?")
        .await
        .unwrap()
        .describe()
        .await
        .unwrap();

    // The query has a single anonymous parameter (or "bind")
    assert!(describe.bind_count() == 1);

    for col in describe.columns() {
        println!("{}: {}", col.name, col.col_type.name());
    }

    // ...
}
```

> `describe()` is best-effort. NetSuite has no describe-only query introspection. As such, I just run your query with a 1-row limit and infers column names/types from that sample row's JSON. `bind_count()` is a client-side count of `?` placeholders in your SQL text, since NetSuite reports no server-side bind metadata either.

### Inserting, Updating, and Deleting Data

As SuiteQL is **read-only**, `INSERT`/`UPDATE`/`DELETE` need to use a different API. To write data, use the NetSuite REST Record API via `conn.records()`:

```rust
use serde_json::json;

async fn main() {
    // ...
    let id = conn
        .records()
        .create("customer", json!({
            "companyName": "Acme Corp",
            "email": "hello@example.com",
        }))
        .await
        .unwrap();

    conn.records()
        .update("customer", &id, json!({ "companyName": "Acme Corporation" }))
        .await
        .unwrap();

    let record = conn.records().get("customer", &id).await.unwrap();

    conn.records().delete("customer", &id).await.unwrap();

    // Create-or-replace by an external id you control:
    let id = conn
        .records()
        .upsert("customer", "my-external-id-123", json!({ "companyName": "Acme Corp" }))
        .await
        .unwrap();

    // ...
}
```

`record_type` is NetSuite's lowercase record type name (`"customer"`, `"salesorder"`, `"inventoryitem"`, etc.) and `fields` follows the JSON shape NetSuite's [Record API](https://docs.oracle.com/en/cloud/saas/netsuite/ns-online-help/section_157909163307.html) documents for that record type. Use nested objects for references and sublists, e.g. `{"entity": {"id": "123"}}`.

## Contributing
PRs are welcomed! Any help is appreciated.

## License
Copyright © 2026, Carl Ian Voller.
