//! Column-name resolution within local schemas and across `SELECT` sources.

use crate::sql::ColumnRef;
use crate::storage::TableSchema;
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ColumnLocation {
    pub(crate) source: usize,
    pub(crate) column: usize,
}

pub(super) fn resolve_column(
    schemas: &[&TableSchema],
    reference: &ColumnRef,
) -> Result<ColumnLocation> {
    if let Some(qualifier) = &reference.qualifier {
        let source = schemas
            .iter()
            .position(|schema| schema.name == *qualifier)
            .ok_or_else(|| Error::schema(format!("unknown table qualifier {qualifier:?}")))?;
        let column = require_column(schemas[source], &reference.name)?;
        return Ok(ColumnLocation { source, column });
    }

    let mut match_ = None;
    for (source, schema) in schemas.iter().enumerate() {
        if let Some(column) = schema
            .columns
            .iter()
            .position(|column| column.name == reference.name)
        {
            if match_.is_some() {
                return Err(Error::schema(format!(
                    "ambiguous column {:?}; qualify it with a table name",
                    reference.name
                )));
            }
            match_ = Some(ColumnLocation { source, column });
        }
    }
    match_.ok_or_else(|| {
        if let [schema] = schemas {
            Error::schema(format!(
                "unknown column {:?} in table {:?}",
                reference.name, schema.name
            ))
        } else {
            Error::schema(format!("unknown column {:?}", reference.name))
        }
    })
}

pub(super) fn require_local_column(schema: &TableSchema, reference: &ColumnRef) -> Result<usize> {
    if let Some(qualifier) = &reference.qualifier
        && qualifier != &schema.name
    {
        return Err(Error::schema(format!(
            "unknown table qualifier {qualifier:?} for table {:?}",
            schema.name
        )));
    }
    require_column(schema, &reference.name)
}

pub(super) fn require_column(schema: &TableSchema, name: &str) -> Result<usize> {
    schema
        .columns
        .iter()
        .position(|column| column.name == name)
        .ok_or_else(|| {
            Error::schema(format!(
                "unknown column {name:?} in table {:?}",
                schema.name
            ))
        })
}
