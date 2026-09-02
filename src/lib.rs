pub(crate) mod auth;
pub mod connection;
pub(crate) mod errors;
pub mod executor;
pub(crate) mod http;
pub mod primitives;
pub mod query;
pub mod record;
pub mod utils;

pub use errors::NetsuiteError;
pub(crate) use errors::{error, this_errors};

pub use connection::{
    NetsuiteConnection, NetsuiteConnectionOpts, NetsuiteConnectionOptsBuilder, NetsuitePool,
};
pub use executor::Executor;
pub use primitives::{
    cell::{Cell, CellValue, ToCellValue},
    column::{Column, ColumnType},
    row::Row,
};
pub use query::{SuiteQlDescribeResult, SuiteQlQuery, SuiteQlQueryResult};
pub use record::RecordClient;
pub use utils::{qualify_table, quote_ident};

#[cfg(test)]
mod tests {}
