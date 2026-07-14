//! A deliberately absurd database whose authoritative state is one UTF-8 string.

mod database;
mod error;
mod limits;
mod regex_plan;
mod resolve;
mod sql;
mod storage;
mod value;

pub use database::Database;
pub use error::{Error, Result, Span};
pub use limits::Limits;
pub use regex_plan::RegexPlan;
pub use value::{Column, DataType, Outcome, RowSet, Value};
