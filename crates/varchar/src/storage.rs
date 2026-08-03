//! Canonical storage and physical edits for the single-string database.

mod budget;
mod candidate;
mod catalog;
mod decode;
mod encode;
mod format;
mod integrity;
mod schema;
mod state;
mod validate;

pub(crate) use candidate::Candidate;
pub(crate) use catalog::{AutoIncrement, Catalog};
pub(crate) use decode::{RowRecordRef, decode_row, row_record};
pub(crate) use encode::{
    MeasuredRowEncoding, ValidatedRowEncoder, encode_cell, encode_row, with_validated_row_encoder,
};
pub(crate) use format::{ROW_PREFIX, encode_text_into};
pub(crate) use schema::{
    ForeignKey, ForeignKeyDeleteAction, ForeignKeyUpdateAction, OwnedValidatedRowLayout, RowLayout,
    TableSchema, ValidatedRowLayout, ValidatedTableSchema, validate_row_layout,
};
#[cfg(test)]
pub(crate) use schema::{reset_row_layout_validations, row_layout_validations};
pub(crate) use state::{EMPTY_BLOB, StorageState};

#[cfg(test)]
mod tests;
