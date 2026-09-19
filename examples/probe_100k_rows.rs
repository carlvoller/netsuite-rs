use futures_util::TryStreamExt;
use netsuite_rs::{Executor, NetsuiteConnectionOptsBuilder};
use std::time::Instant;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opts = NetsuiteConnectionOptsBuilder::default()
        .account_id(std::env::var("NETSUITE_ACCOUNT_ID")?)
        .consumer_key(std::env::var("NETSUITE_CONSUMER_KEY")?)
        .consumer_secret(std::env::var("NETSUITE_CONSUMER_SECRET")?)
        .token_id(std::env::var("NETSUITE_TOKEN_ID")?)
        .token_secret(std::env::var("NETSUITE_TOKEN_SECRET")?)
        .page_size(1000)
        .page_concurrency(8usize)
        .pool_size(2usize)
        .build()?;

    let pool = opts.connect().await?;
    let conn = pool.get().await?;

    println!("== fetching 100,000 rows from transactionline with page_concurrency=8 ==");

    let query = conn
        .query("SELECT id FROM transactionline ORDER BY id FETCH FIRST 100000 ROWS ONLY")
        .await?;

    let results = query.execute().await?;
    println!("expected_result_length = {}", results.expected_result_length());

    let overall_start = Instant::now();
    let mut last_report = Instant::now();
    let mut count: u64 = 0;
    let mut last_id: Option<String> = None;

    let mut rows = results.rows();
    while let Some(row) = rows.try_next().await? {
        count += 1;
        last_id = Some(row.get_by_name("id")?.value.to_string());

        if last_report.elapsed().as_secs() >= 15 {
            println!(
                "  progress: {count} rows in {:?} ({:.1} rows/sec)",
                overall_start.elapsed(),
                count as f64 / overall_start.elapsed().as_secs_f64()
            );
            last_report = Instant::now();
        }
    }

    let elapsed = overall_start.elapsed();
    println!("\n== done ==");
    println!("total rows fetched: {count}");
    println!("last id seen: {}", last_id.unwrap_or_default());
    println!("total time: {elapsed:?}");
    println!("throughput: {:.1} rows/sec", count as f64 / elapsed.as_secs_f64());

    Ok(())
}
