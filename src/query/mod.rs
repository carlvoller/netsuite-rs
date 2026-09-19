use std::{collections::VecDeque, sync::Arc};

use async_stream::try_stream;
use futures_util::{
    StreamExt,
    stream::{BoxStream, FuturesOrdered},
};

use crate::{
    NetsuiteError,
    connection::Connection,
    primitives::{
        cell::{CellValue, ToCellValue, cell_value_to_json},
        column::{Column, infer_columns},
        row::Row,
    },
    utils::count_placeholders,
};

/// A SuiteQL query in progress. Bind parameters positionally with [`bind`](Self::bind) or
/// [`bind_row`](Self::bind_row) (SuiteQL only supports positional `?` placeholders, not named
/// ones), then call [`execute`](Self::execute) or [`describe`](Self::describe).
#[derive(Debug)]
pub struct SuiteQlQuery {
    conn: Connection,
    sql: String,
    params: Vec<CellValue>,
}

impl SuiteQlQuery {
    pub(crate) fn new(query: impl ToString, conn: Connection) -> Self {
        Self {
            conn,
            sql: query.to_string(),
            params: Vec::new(),
        }
    }

    /// Appends a single positional bind parameter.
    pub fn bind(&mut self, value: impl ToCellValue) -> &mut Self {
        self.params.push(value.to_cell_value());
        self
    }

    /// Appends multiple positional bind parameters at once. See the [`row!`](`crate::row`) macro.
    pub fn bind_row(&mut self, params: Vec<impl ToCellValue>) -> &mut Self {
        for param in params {
            self.params.push(param.to_cell_value());
        }
        self
    }

    fn json_params(&self) -> Vec<serde_json::Value> {
        self.params.iter().map(cell_value_to_json).collect()
    }

    /// Runs the query and fetches its first page of results. Call
    /// [`.rows()`](SuiteQlQueryResult::rows) on the result to stream the rest.
    pub async fn execute(self) -> Result<SuiteQlQueryResult, NetsuiteError> {
        let page_size = self.conn.opts().page_size;
        let response = self
            .conn
            .suiteql(&self.sql, self.json_params(), page_size, 0)
            .await?;

        let columns = infer_columns(&response.items);

        Ok(SuiteQlQueryResult {
            conn: self.conn,
            sql: self.sql,
            params: self.params,
            columns,
            total_results: response.total_results,
            first_page_items: response.items,
            next_offset: page_size,
        })
    }

    /// Best-effort description of the query: runs it with a 1-row limit and infers column names
    /// and coarse JSON types from the sample row. `bind_count()` is a client-side count of `?`
    /// placeholders in the SQL text.
    ///
    /// `describe()` is meant to be callable before you know what to bind — that's the point of
    /// `bind_count()` — so any placeholder you haven't already bound with
    /// [`bind`](Self::bind)/[`bind_row`](Self::bind_row) is filled with the number `0` here just
    /// to satisfy NetSuite, which otherwise rejects the request outright for having the wrong
    /// number of parameters (NetSuite has no real describe-only mode, so this has to actually
    /// run the query). `0` is used rather than `NULL` because NetSuite rejects `NULL` as a bind
    /// value outright ("invalid or unsupported search"), whereas `0` is accepted as a bind value
    /// against both numeric and text columns alike.
    ///
    /// Because of that, a query whose placeholders sit inside a filter that a literal `0`
    /// doesn't satisfy (e.g. `WHERE companyname = ?`) may come back with zero sample rows — and
    /// so no inferred columns. Bind representative values yourself first if you need `columns()`
    /// to be populated for a filtered query.
    pub async fn describe(self) -> Result<SuiteQlDescribeResult, NetsuiteError> {
        let bind_count = count_placeholders(&self.sql);

        let mut params = self.json_params();
        while params.len() < bind_count as usize {
            params.push(serde_json::json!(0));
        }

        let response = self.conn.suiteql(&self.sql, params, 1, 0).await?;
        let columns = infer_columns(&response.items);

        Ok(SuiteQlDescribeResult {
            columns,
            bind_count,
        })
    }
}

/// The result of executing a [`SuiteQlQuery`].
#[derive(Debug)]
pub struct SuiteQlQueryResult {
    conn: Connection,
    sql: String,
    params: Vec<CellValue>,
    columns: Vec<Arc<Column>>,
    total_results: i64,
    first_page_items: Vec<serde_json::Value>,
    next_offset: i64,
}

impl SuiteQlQueryResult {
    /// Column names and coarse types, inferred from the first row of the first page. Empty if
    /// the query returned zero rows.
    pub fn columns(&self) -> Vec<Arc<Column>> {
        self.columns.clone()
    }

    /// Always `false`: SuiteQL over this API is read-only, so a `SuiteQlQueryResult` is never a
    /// DML result.
    pub fn is_dml(&self) -> bool {
        false
    }

    /// Always `true`: SuiteQL over this API only ever runs `SELECT` queries.
    pub fn is_dql(&self) -> bool {
        true
    }

    /// Total number of rows NetSuite reports for the query, across all pages.
    pub fn expected_result_length(&self) -> i64 {
        self.total_results
    }

    /// Streams every row, transparently fetching subsequent pages as the stream is consumed.
    ///
    /// Up to the connection's `page_concurrency` remaining pages are fetched concurrently to
    /// overlap their (often NetSuite-side-dominated) latency, but rows are always yielded in
    /// page order regardless of which page's request completes first - a later page's rows
    /// never overtake an earlier page's, exactly as if pages were still fetched one at a time.
    pub fn rows(self) -> BoxStream<'static, Result<Row, NetsuiteError>> {
        let columns = self.columns;
        let conn = self.conn;
        let sql = self.sql;
        let json_params: Vec<serde_json::Value> =
            self.params.iter().map(cell_value_to_json).collect();
        let page_size = conn.opts().page_size;
        let concurrency = conn.opts().page_concurrency.max(1);

        let first_page_items: VecDeque<serde_json::Value> = self.first_page_items.into();

        let mut remaining_offsets: VecDeque<i64> = VecDeque::new();
        let mut offset = self.next_offset;
        while offset < self.total_results {
            remaining_offsets.push_back(offset);
            offset += page_size;
        }

        let stream = try_stream! {
            let mut idx: i64 = 0;

            for item in first_page_items {
                yield Row::new(columns.clone(), item, idx)?;
                idx += 1;
            }

            let spawn_next = |remaining: &mut VecDeque<i64>| {
                remaining.pop_front().map(|offset| {
                    let conn = conn.clone();
                    let sql = sql.clone();
                    let params = json_params.clone();
                    async move { conn.suiteql(&sql, params, page_size, offset).await }
                })
            };

            let mut in_flight = FuturesOrdered::new();
            for _ in 0..concurrency {
                if let Some(fut) = spawn_next(&mut remaining_offsets) {
                    in_flight.push_back(fut);
                }
            }

            while let Some(response) = in_flight.next().await {
                let response = response?;

                if let Some(fut) = spawn_next(&mut remaining_offsets) {
                    in_flight.push_back(fut);
                }

                for item in response.items {
                    yield Row::new(columns.clone(), item, idx)?;
                    idx += 1;
                }
            }
        };

        Box::pin(stream)
    }
}

/// A best-effort description of a [`SuiteQlQuery`], from [`describe`](SuiteQlQuery::describe).
#[derive(Debug)]
pub struct SuiteQlDescribeResult {
    columns: Vec<Arc<Column>>,
    bind_count: i32,
}

impl SuiteQlDescribeResult {
    /// Column names and coarse types, inferred from a 1-row sample execution. Empty if the query
    /// returned zero rows.
    pub fn columns(&self) -> Vec<Arc<Column>> {
        self.columns.clone()
    }

    /// A client-side count of `?` placeholders in the SQL text (outside of string literals).
    /// NetSuite provides no server-side bind metadata to check this against.
    pub fn bind_count(&self) -> i32 {
        self.bind_count
    }

    /// Always `false`: SuiteQL over this API is read-only.
    pub fn is_dml(&self) -> bool {
        false
    }

    /// Always `true`: SuiteQL over this API only ever runs `SELECT` queries.
    pub fn is_dql(&self) -> bool {
        true
    }
}
