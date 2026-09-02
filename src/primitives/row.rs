use std::sync::Arc;

use crate::{NetsuiteError, error};

use super::{
    cell::{Cell, json_to_cell_value},
    column::Column,
};

#[macro_export]
/// Builds a `Vec<CellValue>` from a list of values, for use with
/// [`SuiteQlQuery::bind_row`](`crate::query::SuiteQlQuery::bind_row`).
macro_rules! row {
    ($($val:expr),* $(,)?) => {
        vec![
            $(
                $crate::ToCellValue::to_cell_value($val)
            ),*
        ]
    };
}

/// A single row in a [`SuiteQlQueryResult`](`crate::query::SuiteQlQueryResult`).
#[derive(Debug, Clone)]
pub struct Row {
    columns: Vec<Arc<Column>>,
    values: serde_json::Map<String, serde_json::Value>,
    idx: i64,
}

impl Row {
    pub(crate) fn new(
        columns: Vec<Arc<Column>>,
        value: serde_json::Value,
        idx: i64,
    ) -> Result<Self, NetsuiteError> {
        let values = match value {
            serde_json::Value::Object(map) => map,
            other => {
                return Err(error!(format!(
                    "expected NetSuite row to be a JSON object, got {other}"
                )));
            }
        };

        Ok(Self {
            columns,
            values,
            idx,
        })
    }

    pub fn columns(&self) -> &[Arc<Column>] {
        &self.columns
    }

    /// The row's 0-based position within the overall (paginated) result set.
    pub fn idx(&self) -> i64 {
        self.idx
    }

    pub fn get(&self, idx: usize) -> Result<Cell, NetsuiteError> {
        let col = self
            .columns
            .get(idx)
            .ok_or_else(|| error!(format!("column index {idx} out of bounds")))?
            .clone();

        let value = self
            .values
            .get(&col.name)
            .unwrap_or(&serde_json::Value::Null);

        Ok(Cell {
            value: json_to_cell_value(value),
            col,
        })
    }

    pub fn get_by_name(&self, name: &str) -> Result<Cell, NetsuiteError> {
        let col = self
            .columns
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| error!(format!("no such column: {name}")))?
            .clone();

        let value = self.values.get(name).unwrap_or(&serde_json::Value::Null);

        Ok(Cell {
            value: json_to_cell_value(value),
            col,
        })
    }
}

impl IntoIterator for Row {
    type Item = Cell;
    type IntoIter = std::vec::IntoIter<Cell>;

    fn into_iter(self) -> Self::IntoIter {
        let cells = self
            .columns
            .iter()
            .map(|col| {
                let value = self
                    .values
                    .get(&col.name)
                    .unwrap_or(&serde_json::Value::Null);
                Cell {
                    col: col.clone(),
                    value: json_to_cell_value(value),
                }
            })
            .collect::<Vec<_>>();

        cells.into_iter()
    }
}
