use std::sync::Arc;

/// The coarse JSON type NetSuite returned for a column.
///
/// NetSuite's SuiteQL REST endpoint has no notion of a "describe-only" query and returns plain
/// JSON scalars with no SQL type metadata (a `DATE` and a `TEXT` field are
/// indistinguishable from a `VARCHAR` field, for example). `ColumnType` is therefore only ever
/// the JSON type actually observed on a sample row, not a real NetSuite/Oracle column type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Text,
    Number,
    Boolean,
    /// A nested JSON array or object. SuiteQL result rows are normally flat, so this is rare.
    Json,
    /// The sampled value was `null`, so no type could be inferred.
    Null,
}

impl ColumnType {
    pub fn name(&self) -> &'static str {
        match self {
            ColumnType::Text => "TEXT",
            ColumnType::Number => "NUMBER",
            ColumnType::Boolean => "BOOLEAN",
            ColumnType::Json => "JSON",
            ColumnType::Null => "NULL",
        }
    }
}

/// Describes a column in a [`SuiteQlQueryResult`](`crate::query::SuiteQlQueryResult`) or
/// [`SuiteQlDescribeResult`](`crate::query::SuiteQlDescribeResult`).
///
/// Column names and types are inferred from a sample row returned by NetSuite, not from any
/// authoritative schema/catalog, since SuiteQL exposes none over this API.
#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub col_type: ColumnType,
}

pub(crate) fn infer_columns(items: &[serde_json::Value]) -> Vec<Arc<Column>> {
    let Some(serde_json::Value::Object(first)) = items.first() else {
        return Vec::new();
    };

    first
        .iter()
        .map(|(name, value)| {
            Arc::new(Column {
                name: name.clone(),
                col_type: infer_column_type(value),
            })
        })
        .collect()
}

fn infer_column_type(value: &serde_json::Value) -> ColumnType {
    match value {
        serde_json::Value::Null => ColumnType::Null,
        serde_json::Value::String(_) => ColumnType::Text,
        serde_json::Value::Bool(_) => ColumnType::Boolean,
        serde_json::Value::Number(_) => ColumnType::Number,
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => ColumnType::Json,
    }
}
