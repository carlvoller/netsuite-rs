use futures_util::TryStreamExt;

use crate::{
    NetsuiteError, connection::NetsuiteConnection, primitives::row::Row, query::SuiteQlQuery,
};

// NOTE: I abstracted out the Executor in case I want to implement any possible Transaction
// primitive in the future. However, I'm not entirely sure if this is even possible, but no harm
// abstracting this first.
/// Shared surface for running SuiteQL.
pub trait Executor {
    /// Returns a [`SuiteQlQuery`]. Bind parameters, then call `.execute()` or `.describe()`.
    fn query(
        &self,
        query: impl ToString,
    ) -> impl Future<Output = Result<SuiteQlQuery, NetsuiteError>>;

    /// Runs a query and buffers every row (across all pages) into a `Vec`.
    fn fetch_all(
        &self,
        query: impl ToString,
    ) -> impl Future<Output = Result<Vec<Row>, NetsuiteError>>;

    /// Pings NetSuite. Useful for checking credentials are valid and the account is reachable.
    fn ping(&self) -> impl Future<Output = Result<(), NetsuiteError>>;

    /// Runs a `SELECT` query, discarding its rows. Returns the query's total result count.
    /// Do not try to execute any DML operations using this. SuiteSQL is read-only.
    fn execute(&self, query: impl ToString) -> impl Future<Output = Result<i64, NetsuiteError>>;
}

impl Executor for NetsuiteConnection {
    async fn query(&self, query: impl ToString) -> Result<SuiteQlQuery, NetsuiteError> {
        Ok(SuiteQlQuery::new(query, self.conn.clone()))
    }

    async fn fetch_all(&self, query: impl ToString) -> Result<Vec<Row>, NetsuiteError> {
        let results = self.query(query).await?.execute().await?;
        let capacity = results.expected_result_length().max(0) as usize;
        let mut rows = Vec::with_capacity(capacity);

        let mut stream = results.rows();
        while let Some(row) = stream.try_next().await? {
            rows.push(row);
        }

        Ok(rows)
    }

    async fn ping(&self) -> Result<(), NetsuiteError> {
        self.fetch_all("SELECT 1 FROM DUAL").await?;
        Ok(())
    }

    async fn execute(&self, query: impl ToString) -> Result<i64, NetsuiteError> {
        let results = self.query(query).await?.execute().await?;
        Ok(results.expected_result_length())
    }
}
