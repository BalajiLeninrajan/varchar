//! A deliberately absurd database whose authoritative state is one UTF-8 string.

mod database;
mod error;
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
