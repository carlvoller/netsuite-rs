# netsuite-rs
An async lightweight, familiar driver for [Oracle NetSuite](https://www.netsuite.com/) written **natively** in Rust. This driver uses SuiteQL for reads and the REST Record API for writes. Only Token-Based Authentication is supported.

## Features
- [x] Run SuiteQL queries, with positional bind parameters
- [x] Stream results, transparently paginating past NetSuite's page size
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
netsuite-rs = { version = "1.0.0", features = ["decimal"] }
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
        .pool_size(5usize) // Optional, default 5
        .page_size(1000)   // Optional, default 1000 (NetSuite's max)
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
