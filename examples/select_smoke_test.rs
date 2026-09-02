use futures_util::TryStreamExt;
use netsuite_rs::{Executor, NetsuiteConnectionOptsBuilder};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opts = NetsuiteConnectionOptsBuilder::default()
        .account_id(std::env::var("NETSUITE_ACCOUNT_ID")?)
        .consumer_key(std::env::var("NETSUITE_CONSUMER_KEY")?)
        .consumer_secret(std::env::var("NETSUITE_CONSUMER_SECRET")?)
        .token_id(std::env::var("NETSUITE_TOKEN_ID")?)
        .token_secret(std::env::var("NETSUITE_TOKEN_SECRET")?)
        .page_size(3)
        .pool_size(2usize)
        .build()?;

    let pool = opts.connect().await?;
    let conn = pool.get().await?;

    println!("== ping() ==");
    conn.ping().await?;
    println!("ok\n");

    println!("== describe() ==");
    let mut describe_query = conn
        .query("SELECT id, entityid, companyname FROM customer WHERE id > ?")
        .await?;
    describe_query.bind(0);
    let describe = describe_query.describe().await?;
    println!("bind_count = {}", describe.bind_count());
    println!("is_dml = {}, is_dql = {}", describe.is_dml(), describe.is_dql());
    for col in describe.columns() {
        println!("  column {} ({})", col.name, col.col_type.name());
    }
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

    println!("\nAll SuiteQL SELECT smoke tests passed.");
    Ok(())
}
