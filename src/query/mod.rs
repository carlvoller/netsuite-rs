use std::{collections::VecDeque, sync::Arc};

use async_stream::try_stream;
use futures_util::stream::BoxStream;

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
            has_more: response.has_more,
            next_offset: page_size,
        })
    }

    /// Best-effort description of the query: runs it with a 1-row limit and infers column names
    /// and coarse JSON types from the sample row. `bind_count()` is a client-side count of `?`
    /// placeholders in the SQL text.
    ///
    /// NetSuite reports no real describe-only mode or SQL type metadata, so both figures are
    /// best-effort.
    pub async fn describe(self) -> Result<SuiteQlDescribeResult, NetsuiteError> {
        let bind_count = count_placeholders(&self.sql);
        let response = self
            .conn
            .suiteql(&self.sql, self.json_params(), 1, 0)
            .await?;
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
    has_more: bool,
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

    /// Streams every row, transparently fetching subsequent pages (using the connection's
    /// `page_size`) as the stream is consumed.
    pub fn rows(self) -> BoxStream<'static, Result<Row, NetsuiteError>> {
        let columns = self.columns;
        let conn = self.conn;
        let sql = self.sql;
        let json_params: Vec<serde_json::Value> =
            self.params.iter().map(cell_value_to_json).collect();
        let page_size = conn.opts().page_size;

        let mut items: VecDeque<serde_json::Value> = self.first_page_items.into();
        let mut has_more = self.has_more;
        let mut offset = self.next_offset;
        let mut idx: i64 = 0;

        let stream = try_stream! {
            loop {
                while let Some(item) = items.pop_front() {
                    yield Row::new(columns.clone(), item, idx)?;
                    idx += 1;
                }

                if !has_more {
                    break;
                }

                let response = conn.suiteql(&sql, json_params.clone(), page_size, offset).await?;
                items = response.items.into();
                has_more = response.has_more;
                offset += page_size;
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
