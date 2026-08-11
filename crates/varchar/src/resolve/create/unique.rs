//! Single-column UNIQUE declaration normalization.

use crate::{Error, Result};

pub(super) fn declare_unique(
    column: &str,
    index: usize,
    saw_unique: &mut [bool],
    unique_columns: &mut Vec<usize>,
) -> Result<()> {
    if saw_unique[index] {
        return Err(Error::Schema(format!(
            "duplicate UNIQUE declaration for column {column:?}"
        )));
    }
    saw_unique[index] = true;
    unique_columns.push(index);
    Ok(())
}
