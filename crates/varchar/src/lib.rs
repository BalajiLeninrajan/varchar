//! A deliberately absurd database whose authoritative state is one UTF-8 string.

mod database;
mod error;
mod regex_plan;
mod sql;
mod storage;
mod value;

pub use database::{Database, Limits};
pub use error::{Error, Result, Span};
pub use regex_plan::RegexPlan;
pub use value::{Column, DataType, Outcome, RowSet, Value};
