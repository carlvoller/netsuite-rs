use std::error::Error;
use std::fmt::{Debug, Display};

#[derive(Clone)]
pub struct NetsuiteError {
    trace: String,
    message: String,
    underlying_error: Option<String>,
}

impl NetsuiteError {
    pub(crate) fn new(trace: String, message: String, underlying: Option<String>) -> Self {
        Self {
            trace,
            message,
            underlying_error: underlying,
        }
    }
}

impl Error for NetsuiteError {}

impl Display for NetsuiteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let error_name = self.underlying_error.as_deref().unwrap_or("NetsuiteError");
        write!(f, "[{}] ({}): {}", error_name, self.trace, self.message)
    }
}

impl Debug for NetsuiteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let error_name = self.underlying_error.as_deref().unwrap_or("NetsuiteError");
        write!(f, "[{}] ({}): {}", error_name, self.trace, self.message)
    }
}

/// Builds a [`NetsuiteError`] from a message, and optionally an underlying error to attach for context.
macro_rules! error {
    ($msg:expr) => {
        $crate::errors::NetsuiteError::new(module_path!().to_string(), $msg.to_string(), None)
    };
    ($msg:expr, $err:expr) => {{
        let err = $err;
        let type_name = std::any::type_name_of_val(&err);
        let error_name = type_name.rsplit("::").next().map(|s| s.to_string());
        $crate::errors::NetsuiteError::new(
            module_path!().to_string(),
            format!("{}: {:?}", $msg, err),
            error_name,
        )
    }};
}

/// Runs a fallible expression, converting its error into a [`NetsuiteError`] tagged with `$msg`
/// and propagating it with `?`.
macro_rules! this_errors {
    ($msg:expr, $val:expr) => {
        $val.map_err(|e| $crate::error!($msg, e))?
    };
}

pub(crate) use error;
pub(crate) use this_errors;
