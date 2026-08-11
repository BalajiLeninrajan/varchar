#![doc = include_str!("../README.md")]

/// Long-form documentation lives in `docs/`. Compiling those pages as doctests
/// keeps their examples honest without pulling them into the crate docs.
#[cfg(doctest)]
mod docs {
    #[doc = include_str!("../docs/library-api.md")]
    mod library_api {}
}

mod database;
mod error;
mod expression;
mod limits;
mod output;
mod query;
mod resolve;
mod sql;
mod storage;
mod value;

pub use database::Database;
pub use error::{Error, Result, Span};
pub use limits::{Limits, Resource};
pub use output::{ColumnOrigin, Outcome, ResultColumn, RowSet, SelectExplanation};
pub use value::{DataType, Value};

pub(crate) use value::SchemaColumn;
