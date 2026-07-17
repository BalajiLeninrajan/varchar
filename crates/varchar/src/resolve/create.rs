//! `CREATE TABLE` resolution and validated schema assembly.

mod auto_increment;
mod foreign_key;
mod primary_key;

use std::collections::BTreeSet;

use auto_increment::{declare_auto_increment, validate_auto_increment};
use foreign_key::{declare_foreign_key, validate_foreign_key};
use primary_key::declare_primary_key;

use crate::sql::{ColumnModifier, CreateElement, CreateTable, TableConstraint};
use crate::storage::{Catalog, TableSchema};
use crate::{Error, Result, SchemaColumn};

pub(crate) struct ResolvedCreate {
    pub(crate) schema: TableSchema,
    pub(crate) auto_increment: Option<usize>,
}

pub(crate) fn create_schema(catalog: &Catalog, statement: CreateTable) -> Result<ResolvedCreate> {
    let CreateTable { table, elements } = statement;
    if catalog.table(&table).is_some() {
        return Err(Error::Schema(format!("table {table:?} already exists")));
    }

    // Collect the full column namespace before resolving table constraints.
    // A table constraint may legally precede the column that it names.
    let mut columns = Vec::new();
    let mut column_names = BTreeSet::new();
    for element in &elements {
        let CreateElement::Column(column) = element else {
            continue;
        };
        if !column_names.insert(column.name.clone()) {
            return Err(Error::Schema(format!(
                "duplicate column name {:?}",
                column.name
            )));
        }
        columns.push(SchemaColumn {
            name: column.name.clone(),
            data_type: column.data_type,
            nullable: true,
        });
    }
    if columns.is_empty() {
        return Err(Error::Schema(String::from(
            "table must contain at least one column",
        )));
    }

    let mut primary_key = None;
    let mut foreign_keys = Vec::new();
    let mut auto_increment = None;
    let mut saw_not_null = vec![false; columns.len()];
    let mut saw_foreign_key = vec![false; columns.len()];
    let mut column_index = 0;

    // Fold local declarations in source order. Cross-table and AUTO checks
    // wait until the complete local primary key is available.
    for element in elements {
        match element {
            CreateElement::Column(column) => {
                let index = column_index;
                column_index += 1;
                for modifier in column.modifiers {
                    match modifier {
                        ColumnModifier::NotNull => {
                            if saw_not_null[index] {
                                return Err(Error::Schema(format!(
                                    "duplicate NOT NULL declaration for column {:?}",
                                    column.name
                                )));
                            }
                            saw_not_null[index] = true;
                            columns[index].nullable = false;
                        }
                        ColumnModifier::PrimaryKey => declare_primary_key(
                            &table,
                            &column.name,
                            index,
                            &mut primary_key,
                            &mut columns,
                        )?,
                        ColumnModifier::References(reference) => declare_foreign_key(
                            &column.name,
                            "REFERENCES",
                            index,
                            reference.table,
                            reference.column,
                            &mut saw_foreign_key,
                            &mut foreign_keys,
                        )?,
                        ColumnModifier::AutoIncrement => declare_auto_increment(
                            &table,
                            &column.name,
                            index,
                            &mut auto_increment,
                        )?,
                    }
                }
            }
            CreateElement::Constraint(constraint) => match constraint {
                TableConstraint::PrimaryKey(name) => {
                    let index = local_constraint_column(&columns, &table, &name, "PRIMARY KEY")?;
                    declare_primary_key(&table, &name, index, &mut primary_key, &mut columns)?;
                }
                TableConstraint::ForeignKey { column, reference } => {
                    let index = local_constraint_column(&columns, &table, &column, "FOREIGN KEY")?;
                    declare_foreign_key(
                        &column,
                        "FOREIGN KEY",
                        index,
                        reference.table,
                        reference.column,
                        &mut saw_foreign_key,
                        &mut foreign_keys,
                    )?;
                }
            },
        }
    }

    let mut schema = TableSchema {
        name: table,
        columns,
        primary_key,
        foreign_keys: Vec::new(),
    };
    for foreign_key in &foreign_keys {
        validate_foreign_key(catalog, &schema, foreign_key)?;
    }
    foreign_keys.sort_by_key(|foreign_key| foreign_key.column);
    schema.foreign_keys = foreign_keys;
    if let Some(column) = auto_increment {
        validate_auto_increment(&schema, column)?;
    }
    Ok(ResolvedCreate {
        schema,
        auto_increment,
    })
}

fn local_constraint_column(
    columns: &[SchemaColumn],
    table: &str,
    column: &str,
    constraint: &str,
) -> Result<usize> {
    columns
        .iter()
        .position(|candidate| candidate.name == column)
        .ok_or_else(|| {
            Error::Schema(format!(
                "{constraint} references unknown column {column:?} in table {table:?}"
            ))
        })
}
