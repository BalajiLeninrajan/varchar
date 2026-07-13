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
pub use limits::{Limits, Resource};
pub use query::ExplainPlan;
pub use value::{ColumnOrigin, DataType, Outcome, ResultColumn, RowSet, Value};

pub(crate) use value::SchemaColumn as Column;
