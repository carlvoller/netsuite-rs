use std::{fmt::Display, sync::Arc};

#[cfg(feature = "decimal")]
use bigdecimal::BigDecimal;
#[cfg(feature = "decimal")]
use std::str::FromStr;

use super::column::Column;

#[derive(Debug, Clone)]
/// The value held by a single [`Cell`]. NetSuite's REST APIs speak plain JSON, so this maps
/// directly onto JSON's scalar types rather than onto real Oracle/SuiteQL column types.
pub enum CellValue {
    Text(Option<String>),

    #[cfg(feature = "decimal")]
    Number(Option<BigDecimal>),
    #[cfg(not(feature = "decimal"))]
    Number(Option<f64>),

    Boolean(Option<bool>),

    /// A nested JSON array or object, e.g. from a `SELECT` expression that yields JSON, or a
    /// nested field on a record fetched via the Record API.
    Json(Option<serde_json::Value>),

    Null,
}

#[derive(Debug, Clone)]
/// A single cell in a [`Row`](`crate::primitives::row::Row`)
pub struct Cell {
    pub col: Arc<Column>,
    pub value: CellValue,
}

pub trait ToCellValue {
    fn to_cell_value(self) -> CellValue;
}

macro_rules! impl_numeric_to_cell_value {
    ($($t:ty),*) => {
        $(
            impl ToCellValue for $t {
                fn to_cell_value(self) -> CellValue {
                    #[cfg(feature = "decimal")]
                    {
                        CellValue::Number(BigDecimal::from_str(&self.to_string()).ok())
                    }
                    #[cfg(not(feature = "decimal"))]
                    {
                        CellValue::Number(Some(self as f64))
                    }
                }
            }

            impl ToCellValue for Option<$t> {
                fn to_cell_value(self) -> CellValue {
                    match self {
                        Some(x) => x.to_cell_value(),
                        None => CellValue::Null,
                    }
                }
            }
        )*
    };
}

impl_numeric_to_cell_value!(i8, i16, i32, i64, i128, u8, u16, u32, u64, f32, f64);

impl ToCellValue for bool {
    fn to_cell_value(self) -> CellValue {
        CellValue::Boolean(Some(self))
    }
}

impl ToCellValue for Option<bool> {
    fn to_cell_value(self) -> CellValue {
        CellValue::Boolean(self)
    }
}

impl ToCellValue for &str {
    fn to_cell_value(self) -> CellValue {
        CellValue::Text(Some(self.to_string()))
    }
}

impl ToCellValue for Option<&str> {
    fn to_cell_value(self) -> CellValue {
        CellValue::Text(self.map(|x| x.to_string()))
    }
}

impl ToCellValue for String {
    fn to_cell_value(self) -> CellValue {
        CellValue::Text(Some(self))
    }
}

impl ToCellValue for Option<String> {
    fn to_cell_value(self) -> CellValue {
        CellValue::Text(self)
    }
}

impl ToCellValue for serde_json::Value {
    fn to_cell_value(self) -> CellValue {
        json_to_cell_value(&self)
    }
}

#[cfg(feature = "decimal")]
impl ToCellValue for BigDecimal {
    fn to_cell_value(self) -> CellValue {
        CellValue::Number(Some(self))
    }
}

#[cfg(feature = "decimal")]
impl ToCellValue for Option<BigDecimal> {
    fn to_cell_value(self) -> CellValue {
        CellValue::Number(self)
    }
}

impl ToCellValue for CellValue {
    fn to_cell_value(self) -> CellValue {
        self
    }
}

pub(crate) fn json_to_cell_value(value: &serde_json::Value) -> CellValue {
    match value {
        serde_json::Value::Null => CellValue::Null,
        serde_json::Value::String(s) => CellValue::Text(Some(s.clone())),
        serde_json::Value::Bool(b) => CellValue::Boolean(Some(*b)),
        serde_json::Value::Number(n) => {
            #[cfg(feature = "decimal")]
            {
                CellValue::Number(BigDecimal::from_str(&n.to_string()).ok())
            }
            #[cfg(not(feature = "decimal"))]
            {
                CellValue::Number(n.as_f64())
            }
        }
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            CellValue::Json(Some(value.clone()))
        }
    }
}

pub(crate) fn cell_value_to_json(value: &CellValue) -> serde_json::Value {
    match value {
        CellValue::Text(x) => x
            .clone()
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
        CellValue::Boolean(x) => x
            .map(serde_json::Value::Bool)
            .unwrap_or(serde_json::Value::Null),
        #[cfg(feature = "decimal")]
        CellValue::Number(x) => x
            .as_ref()
            .and_then(|d| serde_json::Number::from_str(&d.to_string()).ok())
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        #[cfg(not(feature = "decimal"))]
        CellValue::Number(x) => x
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        CellValue::Json(x) => x.clone().unwrap_or(serde_json::Value::Null),
        CellValue::Null => serde_json::Value::Null,
    }
}

impl Display for CellValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value_as_string = match self {
            CellValue::Text(x) => x.as_deref().map(|x| x.to_string()),
            CellValue::Number(x) => x.as_ref().map(|x| x.to_string()),
            CellValue::Boolean(x) => x.map(|x| x.to_string()),
            CellValue::Json(value) => value.as_ref().map(|x| x.to_string()),
            CellValue::Null => None,
        };

        write!(f, "{}", value_as_string.unwrap_or("None".into()))
    }
}
