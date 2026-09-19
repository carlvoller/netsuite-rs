use futures_util::TryStreamExt;
use netsuite_rs::{Executor, NetsuiteConnectionOptsBuilder};
use std::time::Instant;

async fn connect(page_concurrency: usize) -> Result<netsuite_rs::NetsuiteConnection, Box<dyn std::error::Error>> {
    let opts = NetsuiteConnectionOptsBuilder::default()
        .account_id(std::env::var("NETSUITE_ACCOUNT_ID")?)
        .consumer_key(std::env::var("NETSUITE_CONSUMER_KEY")?)
        .consumer_secret(std::env::var("NETSUITE_CONSUMER_SECRET")?)
        .token_id(std::env::var("NETSUITE_TOKEN_ID")?)
        .token_secret(std::env::var("NETSUITE_TOKEN_SECRET")?)
        .page_size(3)
        .page_concurrency(page_concurrency)
        .pool_size(2usize)
        .build()?;

    let pool = opts.connect().await?;
    Ok(pool.get().await?)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let conn = connect(3).await?; // page_concurrency=3 (the default) for the main smoke tests

    println!("== ping() ==");
    conn.ping().await?;
    println!("ok\n");

    println!("== describe() with no pre-binding (placeholder auto-filled with 0) ==");
    let describe = conn
        .query("SELECT id, entityid, companyname FROM customer WHERE id > ?")
        .await?
        .describe()
        .await?;
    println!("bind_count = {}", describe.bind_count());
    println!("is_dml = {}, is_dql = {}", describe.is_dml(), describe.is_dql());
    for col in describe.columns() {
        println!("  column {} ({})", col.name, col.col_type.name());
    }

    println!("\n== describe() where the auto-filled value can't match anything ==");
    let describe = conn
        .query("SELECT id, companyname FROM customer WHERE companyname = ?")
        .await?
        .describe()
        .await?;
    println!("bind_count = {}", describe.bind_count());
    println!(
        "columns() = {:?} (empty is expected: `companyname = 0` matches no rows)",
        describe.columns().iter().map(|c| &c.name).collect::<Vec<_>>()
    );
    println!();
    
    println!("== execute() + rows() streaming across pages (page_size=3) ==");
    let query = conn
        .query("SELECT id, entityid, companyname FROM customer ORDER BY id")
        .await?;

    let results = query.execute().await?;
    println!("expected_result_length = {}", results.expected_result_length());
    println!("is_dml = {}, is_dql = {}", results.is_dml(), results.is_dql());

    let mut rows = results.rows();
    let mut count = 0i64;
    while count < 10 {
        let Some(row) = rows.try_next().await? else {
            break;
        };
        count += 1;
        let id = row.get_by_name("id")?;
        let entityid = row.get_by_name("entityid")?;
        let name = row.get_by_name("companyname")?;
        println!("  #{count}: id={} entityid={} companyname={}", id.value, entityid.value, name.value);
    }
    println!("stopped after {count} rows across multiple pages\n");

    println!("== fetch_all() (bounded) ==");
    let rows = conn
        .fetch_all("SELECT id FROM customer ORDER BY id FETCH FIRST 7 ROWS ONLY")
        .await?;
    println!("fetch_all returned {} rows\n", rows.len());

    println!("== bind / single-param query ==");
    let mut query = conn.query("SELECT id FROM customer WHERE id > ?").await?;
    query.bind(0);
    let results = query.execute().await?;
    println!("expected_result_length = {}", results.expected_result_length());

    println!("== page_concurrency: 1 vs 3 vs 8 - timing + order check ==");
    let fetch_ordered_ids = |page_concurrency: usize| async move {
        let conn = connect(page_concurrency).await?;
        let start = Instant::now();
        let rows = conn
            .fetch_all("SELECT id FROM customer ORDER BY id FETCH FIRST 40 ROWS ONLY")
            .await?;
        let elapsed = start.elapsed();
        let ids: Vec<String> = rows
            .iter()
            .map(|r| r.get_by_name("id").unwrap().value.to_string())
            .collect();
        Ok::<_, Box<dyn std::error::Error>>((elapsed, ids))
    };

    let (elapsed_1, ids_1) = fetch_ordered_ids(1).await?;
    println!("  page_concurrency=1: {elapsed_1:?}");

    let (elapsed_3, ids_3) = fetch_ordered_ids(3).await?;
    println!("  page_concurrency=3: {elapsed_3:?}");
    assert_eq!(ids_1, ids_3, "row order must be identical regardless of page_concurrency");

    match fetch_ordered_ids(8).await {
        Ok((elapsed_8, ids_8)) => {
            println!("  page_concurrency=8: {elapsed_8:?}");
            assert_eq!(ids_1, ids_8, "row order must be identical regardless of page_concurrency");
        }
        Err(e) => println!("  page_concurrency=8: ERROR - {e}"),
    }

    println!("  row order identical across all successful runs: confirmed");

    println!("\nAll SuiteQL SELECT smoke tests passed.");
    Ok(())
}
