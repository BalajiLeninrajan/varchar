//! Canonical metadata phase validation and catalog reconstruction.

use std::collections::BTreeMap;
use std::ops::Range;

use super::super::catalog::AutoIncrementState;
use super::super::decode::{AutoIncrementMetadata, ForeignKeyMetadata, PrimaryKeyMetadata};
use super::super::{Catalog, ForeignKey, TableSchema};
use super::ValidationMode;
use crate::{DataType, Error, Result};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MetadataPhase {
    None,
    Keys,
    AutoIncrement,
}

struct MetadataState {
    table: String,
    phase: MetadataPhase,
    saw_primary_key: bool,
    saw_foreign_key: bool,
    next_foreign_key_column: usize,
}

impl MetadataState {
    fn none() -> Self {
        Self {
            table: String::new(),
            phase: MetadataPhase::None,
            saw_primary_key: false,
            saw_foreign_key: false,
            next_foreign_key_column: 0,
        }
    }

    fn begin_table(table: &str) -> Self {
        Self {
            table: table.to_owned(),
            phase: MetadataPhase::Keys,
            saw_primary_key: false,
            saw_foreign_key: false,
            next_foreign_key_column: 0,
        }
    }
}

pub(super) struct MetadataValidator {
    tables: BTreeMap<String, TableSchema>,
    auto_increments: BTreeMap<String, AutoIncrementState>,
    state: MetadataState,
}

impl MetadataValidator {
    pub(super) fn new() -> Self {
        Self {
            tables: BTreeMap::new(),
            auto_increments: BTreeMap::new(),
            state: MetadataState::none(),
        }
    }

    pub(super) fn table(&self, name: &str) -> Option<&TableSchema> {
        self.tables.get(name)
    }

    pub(super) fn insert_schema(&mut self, schema: TableSchema, offset: usize) -> Result<()> {
        if self.tables.contains_key(&schema.name) {
            return Err(super::super::format::corrupt(
                offset,
                "duplicate table schema",
            ));
        }
        self.state = MetadataState::begin_table(&schema.name);
        self.tables.insert(schema.name.clone(), schema);
        Ok(())
    }

    pub(super) fn apply_primary_key(
        &mut self,
        metadata: PrimaryKeyMetadata<'_>,
        offset: usize,
        mode: ValidationMode,
    ) -> Result<()> {
        let result = self.apply_primary_key_inner(metadata, offset);
        result.map_err(|violation| violation.into_error(mode))
    }

    pub(super) fn apply_foreign_key(
        &mut self,
        metadata: ForeignKeyMetadata<'_>,
        offset: usize,
        mode: ValidationMode,
    ) -> Result<()> {
        let result = self.apply_foreign_key_inner(metadata, offset);
        result.map_err(|violation| violation.into_error(mode))
    }

    pub(super) fn apply_auto_increment(
        &mut self,
        metadata: AutoIncrementMetadata<'_>,
        record_range: Range<usize>,
        mode: ValidationMode,
    ) -> Result<()> {
        let result = self.apply_auto_increment_inner(metadata, record_range);
        result.map_err(|violation| violation.into_error(mode))
    }

    pub(super) fn finish(self, row_start: usize) -> Catalog {
        Catalog {
            tables: self.tables,
            auto_increments: self.auto_increments,
            row_start,
        }
    }

    fn apply_primary_key_inner(
        &mut self,
        metadata: PrimaryKeyMetadata<'_>,
        offset: usize,
    ) -> std::result::Result<(), Violation> {
        if self.state.phase != MetadataPhase::Keys
            || self.state.saw_primary_key
            || self.state.saw_foreign_key
            || metadata.table != self.state.table
        {
            return Err(Violation::new(
                offset,
                "primary-key metadata must immediately follow its table schema",
            ));
        }
        let schema = self
            .tables
            .get_mut(&self.state.table)
            .expect("metadata state always names the most recent schema");
        let Some(column) = schema
            .columns
            .iter()
            .position(|column| column.name == metadata.column)
        else {
            return Err(Violation::new(
                offset,
                format!(
                    "primary key for table {:?} references unknown column {:?}",
                    metadata.table, metadata.column
                ),
            ));
        };
        if schema.columns[column].nullable {
            return Err(Violation::new(
                offset,
                format!(
                    "primary-key column {:?}.{:?} must be NOT NULL",
                    metadata.table, metadata.column
                ),
            ));
        }
        schema.primary_key = Some(column);
        self.state.saw_primary_key = true;
        Ok(())
    }

    fn apply_foreign_key_inner(
        &mut self,
        metadata: ForeignKeyMetadata<'_>,
        offset: usize,
    ) -> std::result::Result<(), Violation> {
        if self.state.phase != MetadataPhase::Keys || metadata.table != self.state.table {
            return Err(Violation::new(
                offset,
                "foreign-key metadata must follow its table schema and primary key",
            ));
        }

        let (column, data_type) = {
            let schema = self
                .tables
                .get(&self.state.table)
                .expect("metadata state always names the most recent schema");
            let remaining_columns = &schema.columns[self.state.next_foreign_key_column..];
            let Some(relative_column) = remaining_columns
                .iter()
                .position(|column| column.name == metadata.column)
            else {
                let message = if schema.columns[..self.state.next_foreign_key_column]
                    .iter()
                    .any(|column| column.name == metadata.column)
                {
                    String::from("foreign-key metadata is not in increasing local-column order")
                } else {
                    format!(
                        "foreign key for table {:?} references unknown local column {:?}",
                        metadata.table, metadata.column
                    )
                };
                return Err(Violation::new(offset, message));
            };
            let column = self.state.next_foreign_key_column + relative_column;
            (column, schema.columns[column].data_type)
        };

        let Some(referenced_schema) = self.tables.get(metadata.referenced_table) else {
            return Err(Violation::new(
                offset,
                format!(
                    "foreign key references unknown or later table {:?}",
                    metadata.referenced_table
                ),
            ));
        };
        let Some(referenced_column) = referenced_schema.primary_key else {
            return Err(Violation::new(
                offset,
                format!(
                    "foreign key target {:?}.{:?} is not its table's primary key",
                    metadata.referenced_table, metadata.referenced_column
                ),
            ));
        };
        if referenced_schema.columns[referenced_column].name != metadata.referenced_column {
            return Err(Violation::new(
                offset,
                format!(
                    "foreign key target {:?}.{:?} is not its table's primary key",
                    metadata.referenced_table, metadata.referenced_column
                ),
            ));
        }
        if data_type != referenced_schema.columns[referenced_column].data_type {
            return Err(Violation::new(
                offset,
                format!(
                    "foreign-key columns {:?}.{:?} and {:?}.{:?} have different types",
                    metadata.table,
                    metadata.column,
                    metadata.referenced_table,
                    metadata.referenced_column
                ),
            ));
        }

        self.tables
            .get_mut(&self.state.table)
            .expect("metadata state always names the most recent schema")
            .foreign_keys
            .push(ForeignKey {
                column,
                referenced_table: metadata.referenced_table.to_owned(),
                referenced_column: metadata.referenced_column.to_owned(),
            });
        self.state.saw_foreign_key = true;
        self.state.next_foreign_key_column = column + 1;
        Ok(())
    }

    fn apply_auto_increment_inner(
        &mut self,
        metadata: AutoIncrementMetadata<'_>,
        record_range: Range<usize>,
    ) -> std::result::Result<(), Violation> {
        let offset = record_range.start;
        if self.state.phase != MetadataPhase::Keys
            || (!self.state.saw_primary_key && !self.state.saw_foreign_key)
            || metadata.table != self.state.table
        {
            return Err(Violation::new(
                offset,
                "auto-increment metadata must follow its table's primary and foreign keys",
            ));
        }
        if metadata.last < 0 {
            return Err(Violation::new(
                offset,
                "auto-increment high-water mark must be nonnegative",
            ));
        }
        if self.auto_increments.contains_key(metadata.table) {
            return Err(Violation::new(offset, "duplicate auto-increment metadata"));
        }

        let schema = self
            .tables
            .get(&self.state.table)
            .expect("metadata state always names the most recent schema");
        let Some(primary_key) = schema.primary_key else {
            return Err(Violation::new(
                offset,
                "auto-increment column must be the table's INTEGER primary key",
            ));
        };
        let column = &schema.columns[primary_key];
        if column.name != metadata.column || column.data_type != DataType::Integer {
            return Err(Violation::new(
                offset,
                format!(
                    "auto-increment column {:?}.{:?} must be its INTEGER primary key",
                    metadata.table, metadata.column
                ),
            ));
        }

        self.auto_increments.insert(
            metadata.table.to_owned(),
            AutoIncrementState {
                column: primary_key,
                last: metadata.last,
                record_range,
            },
        );
        self.state.phase = MetadataPhase::AutoIncrement;
        Ok(())
    }
}

pub(super) struct Violation {
    pub(super) offset: usize,
    pub(super) message: String,
}

impl Violation {
    fn new(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
        }
    }

    pub(super) fn into_error(self, mode: ValidationMode) -> Error {
        map_schema_violation_parts(self.offset, self.message, mode)
    }
}

fn map_schema_violation_parts(offset: usize, message: String, mode: ValidationMode) -> Error {
    match mode {
        ValidationMode::Persisted => super::super::format::corrupt(offset, message),
        ValidationMode::Candidate => Error::Schema(message),
    }
}
