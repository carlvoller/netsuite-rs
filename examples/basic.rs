use futures_util::TryStreamExt;
use netsuite_rs::{Executor, NetsuiteConnectionOptsBuilder, row};
use serde_json::json;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opts = NetsuiteConnectionOptsBuilder::default()
        .account_id(std::env::var("NETSUITE_ACCOUNT_ID")?)
        .consumer_key(std::env::var("NETSUITE_CONSUMER_KEY")?)
        .consumer_secret(std::env::var("NETSUITE_CONSUMER_SECRET")?)
        .token_id(std::env::var("NETSUITE_TOKEN_ID")?)
        .token_secret(std::env::var("NETSUITE_TOKEN_SECRET")?)
        .pool_size(5usize)
        .build()?;

    let pool = opts.connect().await?;
    let conn = pool.get().await?;

    // Describe a query without fetching every row.
    let describe = conn
        .query("SELECT id, entityid, companyname FROM customer WHERE id > ?")
        .await?
        .describe()
        .await?;

    println!("bind_count = {}", describe.bind_count());
    for col in describe.columns() {
        println!("column {} ({})", col.name, col.col_type.name());
    }

    // Run a query and stream its rows, auto-paginating past NetSuite's page size.
    let mut query = conn
        .query("SELECT id, entityid, companyname FROM customer WHERE id > ?")
        .await?;
    query.bind(100);

    let results = query.execute().await?;
    println!("total rows = {}", results.expected_result_length());

    let mut rows = results.rows();
    while let Some(row) = rows.try_next().await? {
        let id = row.get_by_name("id")?;
        let name = row.get_by_name("companyname")?;
        println!("{} - {}", id.value, name.value);
    }

    // Or just buffer everything into a Vec via the Executor trait.
    let all_rows = conn
        .fetch_all("SELECT id FROM customer WHERE id > 100")
        .await?;
    println!("fetched {} rows", all_rows.len());

    // Insert/update/delete go through the Record API, since SuiteQL is read-only.
    let new_id = conn
        .records()
        .create(
            "customer",
            json!({
                "companyName": "Acme Corp",
                "email": "hello@example.com",
            }),
        )
        .await?;
    println!("created customer {new_id}");

    conn.records()
        .update(
            "customer",
            &new_id,
            json!({ "companyName": "Acme Corporation" }),
        )
        .await?;

    let fetched = conn.records().get("customer", &new_id).await?;
    println!("fetched record: {fetched}");

    conn.records().delete("customer", &new_id).await?;

    // `row!` is handy for building a bind list up front instead of calling `.bind()` repeatedly.
    let mut query = conn
        .query("SELECT id FROM customer WHERE id > ? AND id < ?")
        .await?;
    query.bind_row(row![100, 200]);
    query.execute().await?;

    Ok(())
}
