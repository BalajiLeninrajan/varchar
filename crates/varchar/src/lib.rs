//! A deliberately absurd database whose authoritative state is one UTF-8 string.

mod database;
mod error;
mod limits;
mod query;
mod resolve;
mod sql;
mod storage;
mod value;

pub use database::Database;
pub use error::{Error, Result, Span};
pub use limits::Limits;
pub use value::{ColumnOrigin, DataType, Outcome, ResultColumn, RowSet, SelectExplanation, Value};

pub(crate) use value::SchemaColumn;
