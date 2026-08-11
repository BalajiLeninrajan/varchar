//! Canonical metadata phase validation and catalog reconstruction.

mod check;

use std::ops::Range;

use super::super::catalog::{AutoIncrementState, CatalogMap};
use super::super::decode::{
    AutoIncrementMetadata, CheckMetadata, DefaultMetadata, ForeignKeyMetadata, PrimaryKeyMetadata,
    UniqueMetadata, decode_cell_at, validate_cell_at,
};
use super::super::{Catalog, ForeignKey, ForeignKeyDeleteAction, TableSchema};
use super::ValidationMode;
use crate::expression::CheckProgram;
use crate::limits::ByteBudget;
use crate::{DataType, Error, Result};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MetadataPhase {
    None,
    Keys,
    AutoIncrement,
    Defaults,
    Unique,
    Checks,
}

struct MetadataState {
    table: String,
    phase: MetadataPhase,
    saw_primary_key: bool,
    saw_foreign_key: bool,
    next_foreign_key_column: usize,
    next_default_column: usize,
    next_unique_column: usize,
    check_predicates: usize,
}

impl MetadataState {
    fn none() -> Self {
        Self {
            table: String::new(),
            phase: MetadataPhase::None,
            saw_primary_key: false,
            saw_foreign_key: false,
            next_foreign_key_column: 0,
            next_default_column: 0,
            next_unique_column: 0,
            check_predicates: 0,
        }
    }

    fn begin_table(table: String) -> Self {
        Self {
            table,
            phase: MetadataPhase::Keys,
            saw_primary_key: false,
            saw_foreign_key: false,
            next_foreign_key_column: 0,
            next_default_column: 0,
            next_unique_column: 0,
            check_predicates: 0,
        }
    }
}

pub(super) struct MetadataValidator {
    tables: CatalogMap<TableSchema>,
    auto_increments: CatalogMap<AutoIncrementState>,
    state: MetadataState,
}

impl MetadataValidator {
    pub(super) fn new() -> Self {
        Self {
            tables: CatalogMap::new(),
            auto_increments: CatalogMap::new(),
            state: MetadataState::none(),
        }
    }

    pub(super) fn table(&self, name: &str) -> Option<&TableSchema> {
        self.tables.get(name)
    }

    pub(super) fn insert_schema(
        &mut self,
        schema: TableSchema,
        offset: usize,
        budget: &mut ByteBudget,
    ) -> Result<()> {
        if self.tables.contains_key(&schema.name) {
            return Err(super::super::format::corrupt(
                offset,
                "duplicate table schema",
            ));
        }
        let state_table =
            budget.clone_text(&schema.name, "allocating the active metadata table name")?;
        let map_key = budget.clone_text(&schema.name, "allocating a catalog table key")?;
        self.state = MetadataState::begin_table(state_table);
        self.tables.insert_new(
            map_key,
            schema,
            budget,
            "reserving the reconstructed table catalog",
        )?;
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
        budget: &mut ByteBudget,
    ) -> Result<()> {
        let result = self.apply_foreign_key_inner(metadata, offset, budget);
        result.map_err(|violation| violation.into_error(mode))
    }

    pub(super) fn apply_auto_increment(
        &mut self,
        metadata: AutoIncrementMetadata<'_>,
        record_range: Range<usize>,
        mode: ValidationMode,
        budget: &mut ByteBudget,
    ) -> Result<()> {
        let result = self.apply_auto_increment_inner(metadata, record_range, budget);
        result.map_err(|violation| violation.into_error(mode))
    }

    pub(super) fn apply_default(
        &mut self,
        metadata: DefaultMetadata<'_>,
        offset: usize,
        mode: ValidationMode,
        budget: &mut ByteBudget,
    ) -> Result<()> {
        let result = self.apply_default_inner(metadata, offset, budget);
        result.map_err(|violation| violation.into_error(mode))
    }

    pub(super) fn apply_unique(
        &mut self,
        metadata: UniqueMetadata<'_>,
        offset: usize,
        mode: ValidationMode,
        budget: &mut ByteBudget,
    ) -> Result<()> {
        let result = self.apply_unique_inner(metadata, offset, budget);
        result.map_err(|violation| violation.into_error(mode))
    }

    pub(super) fn apply_check(
        &mut self,
        metadata: CheckMetadata<'_>,
        offset: usize,
        mode: ValidationMode,
        max_predicates: usize,
        budget: &mut ByteBudget,
    ) -> Result<()> {
        let result = self.apply_check_inner(metadata, offset, max_predicates, budget);
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
        budget: &mut ByteBudget,
    ) -> std::result::Result<(), Violation> {
        if self.state.phase != MetadataPhase::Keys || metadata.table != self.state.table {
            return Err(Violation::new(
                offset,
                "foreign-key metadata must follow its table schema and primary key",
            ));
        }

        let (column, data_type, nullable) = {
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
            (
                column,
                schema.columns[column].data_type,
                schema.columns[column].nullable,
            )
        };

        if metadata.on_delete == ForeignKeyDeleteAction::SetNull && !nullable {
            return Err(Violation::new(
                offset,
                format!(
                    "ON DELETE SET NULL requires nullable foreign-key column {:?}.{:?}",
                    metadata.table, metadata.column
                ),
            ));
        }

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

        let referenced_table = budget
            .clone_text(
                metadata.referenced_table,
                "allocating a foreign-key table name",
            )
            .map_err(Violation::storage)?;
        let referenced_column = budget
            .clone_text(
                metadata.referenced_column,
                "allocating a foreign-key column name",
            )
            .map_err(Violation::storage)?;
        let foreign_keys = &mut self
            .tables
            .get_mut(&self.state.table)
            .expect("metadata state always names the most recent schema")
            .foreign_keys;
        budget
            .reserve_exact(foreign_keys, 1, "reserving decoded foreign-key metadata")
            .map_err(Violation::storage)?;
        foreign_keys.push(ForeignKey {
            column,
            referenced_table,
            referenced_column,
            on_delete: metadata.on_delete,
            on_update: metadata.on_update,
        });
        self.state.saw_foreign_key = true;
        self.state.next_foreign_key_column = column + 1;
        Ok(())
    }

    fn apply_auto_increment_inner(
        &mut self,
        metadata: AutoIncrementMetadata<'_>,
        record_range: Range<usize>,
        budget: &mut ByteBudget,
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

        let table = budget
            .clone_text(metadata.table, "allocating an auto-increment catalog key")
            .map_err(Violation::storage)?;
        self.auto_increments
            .insert_new(
                table,
                AutoIncrementState {
                    column: primary_key,
                    last: metadata.last,
                    record_range,
                },
                budget,
                "reserving the reconstructed auto-increment catalog",
            )
            .map_err(Violation::storage)?;
        self.state.phase = MetadataPhase::AutoIncrement;
        Ok(())
    }

    fn apply_default_inner(
        &mut self,
        metadata: DefaultMetadata<'_>,
        offset: usize,
        budget: &mut ByteBudget,
    ) -> std::result::Result<(), Violation> {
        if !matches!(
            self.state.phase,
            MetadataPhase::Keys | MetadataPhase::AutoIncrement | MetadataPhase::Defaults
        ) || metadata.table != self.state.table
        {
            return Err(Violation::new(
                offset,
                "DEFAULT metadata is outside its table's DEFAULT phase",
            ));
        }

        let schema = self
            .tables
            .get(&self.state.table)
            .expect("metadata state always names the most recent schema");
        let remaining_columns = &schema.columns[self.state.next_default_column..];
        let Some(relative_column) = remaining_columns
            .iter()
            .position(|column| column.name == metadata.column)
        else {
            let message = if schema.columns[..self.state.next_default_column]
                .iter()
                .any(|column| column.name == metadata.column)
            {
                String::from("DEFAULT metadata is duplicated or not in increasing column order")
            } else {
                format!(
                    "DEFAULT for table {:?} references unknown column {:?}",
                    metadata.table, metadata.column
                )
            };
            return Err(Violation::new(offset, message));
        };
        let column = self.state.next_default_column + relative_column;
        if self
            .auto_increments
            .get(metadata.table)
            .is_some_and(|state| state.column == column)
        {
            return Err(Violation::new(
                offset,
                "auto-increment columns cannot have DEFAULT metadata",
            ));
        }

        let definition = &schema.columns[column];
        validate_cell_at(metadata.encoded_value, definition, metadata.value_offset)
            .map_err(Violation::storage)?;
        budget
            .charge(metadata.encoded_value.len())
            .map_err(Violation::storage)?;
        let value = decode_cell_at(metadata.encoded_value, definition, metadata.value_offset)
            .map_err(Violation::storage)?;
        self.tables
            .get_mut(&self.state.table)
            .expect("metadata state always names the most recent schema")
            .columns[column]
            .default = Some(value);
        self.state.phase = MetadataPhase::Defaults;
        self.state.next_default_column = column + 1;
        Ok(())
    }

    fn apply_unique_inner(
        &mut self,
        metadata: UniqueMetadata<'_>,
        offset: usize,
        budget: &mut ByteBudget,
    ) -> std::result::Result<(), Violation> {
        if !matches!(
            self.state.phase,
            MetadataPhase::Keys
                | MetadataPhase::AutoIncrement
                | MetadataPhase::Defaults
                | MetadataPhase::Unique
        ) || metadata.table != self.state.table
        {
            return Err(Violation::new(
                offset,
                "UNIQUE metadata is outside its table's UNIQUE phase",
            ));
        }

        let schema = self
            .tables
            .get(&self.state.table)
            .expect("metadata state always names the most recent schema");
        let remaining_columns = &schema.columns[self.state.next_unique_column..];
        let Some(relative_column) = remaining_columns
            .iter()
            .position(|column| column.name == metadata.column)
        else {
            let message = if schema.columns[..self.state.next_unique_column]
                .iter()
                .any(|column| column.name == metadata.column)
            {
                String::from("UNIQUE metadata is duplicated or not in increasing column order")
            } else {
                format!(
                    "UNIQUE for table {:?} references unknown column {:?}",
                    metadata.table, metadata.column
                )
            };
            return Err(Violation::new(offset, message));
        };
        let column = self.state.next_unique_column + relative_column;
        if schema.primary_key == Some(column) {
            return Err(Violation::new(
                offset,
                "UNIQUE metadata must not duplicate a primary key",
            ));
        }

        let unique_columns = &mut self
            .tables
            .get_mut(&self.state.table)
            .expect("metadata state always names the most recent schema")
            .unique_columns;
        reserve_unique_column(unique_columns, budget).map_err(Violation::storage)?;
        unique_columns.push(column);
        self.state.phase = MetadataPhase::Unique;
        self.state.next_unique_column = column + 1;
        Ok(())
    }

    fn apply_check_inner(
        &mut self,
        metadata: CheckMetadata<'_>,
        offset: usize,
        max_predicates: usize,
        budget: &mut ByteBudget,
    ) -> std::result::Result<(), Violation> {
        if !matches!(
            self.state.phase,
            MetadataPhase::Keys
                | MetadataPhase::AutoIncrement
                | MetadataPhase::Defaults
                | MetadataPhase::Unique
                | MetadataPhase::Checks
        ) || metadata.table != self.state.table
        {
            return Err(Violation::new(
                offset,
                "CHECK metadata is outside its table's CHECK phase",
            ));
        }

        let schema = self
            .tables
            .get(&self.state.table)
            .expect("metadata state always names the most recent schema");
        let (program, predicates) = check::decode_program(
            schema,
            metadata,
            self.state.check_predicates,
            max_predicates,
            budget,
        )
        .map_err(Violation::storage)?;

        let checks = &mut self
            .tables
            .get_mut(&self.state.table)
            .expect("metadata state always names the most recent schema")
            .checks;
        reserve_check_program(checks, budget).map_err(Violation::storage)?;
        checks.push(program);
        self.state.phase = MetadataPhase::Checks;
        self.state.check_predicates = predicates;
        Ok(())
    }
}

fn reserve_check_program(checks: &mut Vec<CheckProgram>, budget: &mut ByteBudget) -> Result<()> {
    const OPERATION: &str = "reserving decoded CHECK metadata";

    budget.charge_items::<CheckProgram>(1)?;
    if checks.len() == checks.capacity() {
        let additional = checks.capacity().max(1);
        checks
            .capacity()
            .checked_add(additional)
            .ok_or(Error::Capacity {
                operation: OPERATION,
            })?;
        checks
            .try_reserve_exact(additional)
            .map_err(|_| Error::Allocation {
                operation: OPERATION,
            })?;
    }
    Ok(())
}

fn reserve_unique_column(columns: &mut Vec<usize>, budget: &mut ByteBudget) -> Result<()> {
    const OPERATION: &str = "reserving decoded UNIQUE metadata";

    budget.charge_items::<usize>(1)?;
    if columns.len() == columns.capacity() {
        let additional = columns.capacity().max(1);
        columns
            .capacity()
            .checked_add(additional)
            .ok_or(Error::Capacity {
                operation: OPERATION,
            })?;
        columns
            .try_reserve_exact(additional)
            .map_err(|_| Error::Allocation {
                operation: OPERATION,
            })?;
    }
    Ok(())
}

pub(super) struct Violation {
    pub(super) offset: usize,
    pub(super) message: String,
    storage: Option<Error>,
}

impl Violation {
    fn new(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
            storage: None,
        }
    }

    fn storage(error: Error) -> Self {
        Self {
            offset: 0,
            message: String::new(),
            storage: Some(error),
        }
    }

    pub(super) fn into_error(self, mode: ValidationMode) -> Error {
        self.storage
            .unwrap_or_else(|| map_schema_violation_parts(self.offset, self.message, mode))
    }
}

fn map_schema_violation_parts(offset: usize, message: String, mode: ValidationMode) -> Error {
    match mode {
        ValidationMode::Persisted => super::super::format::corrupt(offset, message),
        ValidationMode::Candidate => Error::Schema(message),
    }
}
