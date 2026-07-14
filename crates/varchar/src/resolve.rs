//! Schema-aware SQL name and type resolution.
//!
//! This layer turns parser-owned names into column positions and validates
//! logical values. It deliberately knows nothing about storage encodings,
//! regular expressions, row scans, or candidate commits.

use std::collections::BTreeSet;

use crate::sql::{
    Assignment, ColumnModifier, CreateElement, CreateTable, Predicate, PredicateOperator,
    Projection, TableConstraint,
};
use crate::storage::{AutoIncrement, Catalog, ForeignKey, TableSchema};
use crate::value::validate_value;
use crate::{Column, DataType, Error, Result, Value};

pub(crate) enum ResolvedPredicate<'a> {
    Equal { column: usize, value: &'a Value },
    NotEqual { column: usize, value: &'a Value },
    Like { column: usize, pattern: &'a str },
    IsNull { column: usize },
    IsNotNull { column: usize },
}

pub(crate) struct ResolvedCreate {
    pub(crate) schema: TableSchema,
    pub(crate) auto_increment: Option<usize>,
}

pub(crate) struct ResolvedInsert {
    pub(crate) values: Vec<Value>,
    pub(crate) next_auto_increment: Option<i64>,
}

pub(crate) struct ResolvedAssignments {
    pub(crate) values: Vec<(usize, Value)>,
    pub(crate) next_auto_increment: Option<i64>,
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
        columns.push(Column {
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

fn declare_primary_key(
    table: &str,
    column: &str,
    index: usize,
    primary_key: &mut Option<usize>,
    columns: &mut [Column],
) -> Result<()> {
    match *primary_key {
        Some(existing) if existing == index => {
            return Err(Error::Schema(format!(
                "duplicate PRIMARY KEY declaration for column {column:?}"
            )));
        }
        Some(_) => return Err(multiple_primary_keys(table)),
        None => *primary_key = Some(index),
    }
    columns[index].nullable = false;
    Ok(())
}

fn declare_foreign_key(
    column: &str,
    syntax: &str,
    index: usize,
    referenced_table: String,
    referenced_column: String,
    saw_foreign_key: &mut [bool],
    foreign_keys: &mut Vec<ForeignKey>,
) -> Result<()> {
    if saw_foreign_key[index] {
        return Err(Error::Schema(format!(
            "duplicate {syntax} declaration for column {column:?}"
        )));
    }
    saw_foreign_key[index] = true;
    foreign_keys.push(ForeignKey {
        column: index,
        referenced_table,
        referenced_column,
    });
    Ok(())
}

fn declare_auto_increment(
    table: &str,
    column: &str,
    index: usize,
    auto_increment: &mut Option<usize>,
) -> Result<()> {
    match *auto_increment {
        Some(existing) if existing == index => Err(Error::Schema(format!(
            "duplicate AUTOINCREMENT declaration for column {column:?}"
        ))),
        Some(_) => Err(Error::Schema(format!(
            "table {table:?} may have only one auto-increment column"
        ))),
        None => {
            *auto_increment = Some(index);
            Ok(())
        }
    }
}

fn validate_foreign_key(
    catalog: &Catalog,
    schema: &TableSchema,
    foreign_key: &ForeignKey,
) -> Result<()> {
    let referenced_schema = if foreign_key.referenced_table == schema.name {
        schema
    } else {
        catalog
            .table(&foreign_key.referenced_table)
            .ok_or_else(|| {
                Error::Schema(format!(
                    "foreign key references unknown or later table {:?}",
                    foreign_key.referenced_table
                ))
            })?
    };
    let referenced_primary_key = referenced_schema
        .primary_key
        .filter(|&index| referenced_schema.columns[index].name == foreign_key.referenced_column);
    let Some(referenced_primary_key) = referenced_primary_key else {
        return Err(Error::Schema(format!(
            "foreign key target {:?}.{:?} is not its table's primary key",
            foreign_key.referenced_table, foreign_key.referenced_column
        )));
    };
    if schema.columns[foreign_key.column].data_type
        != referenced_schema.columns[referenced_primary_key].data_type
    {
        return Err(Error::Schema(format!(
            "foreign-key columns {:?}.{:?} and {:?}.{:?} have different types",
            schema.name,
            schema.columns[foreign_key.column].name,
            foreign_key.referenced_table,
            foreign_key.referenced_column
        )));
    }
    Ok(())
}

fn validate_auto_increment(schema: &TableSchema, column: usize) -> Result<()> {
    let definition = &schema.columns[column];
    if schema.primary_key != Some(column) || definition.data_type != DataType::Integer {
        return Err(Error::Schema(format!(
            "auto-increment column {:?}.{:?} must be its INTEGER primary key",
            schema.name, definition.name
        )));
    }
    Ok(())
}

fn local_constraint_column(
    columns: &[Column],
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

fn multiple_primary_keys(table: &str) -> Error {
    Error::Schema(format!(
        "table {table:?} may have only one PRIMARY KEY column"
    ))
}

pub(crate) fn require_table<'a>(catalog: &'a Catalog, table: &str) -> Result<&'a TableSchema> {
    catalog
        .table(table)
        .ok_or_else(|| Error::Schema(format!("unknown table {table:?}")))
}

pub(crate) fn projection(schema: &TableSchema, projection: &Projection) -> Result<Vec<usize>> {
    match projection {
        Projection::All => Ok((0..schema.columns.len()).collect()),
        Projection::Columns(columns) => columns
            .iter()
            .map(|name| require_column(schema, name))
            .collect(),
    }
}

pub(crate) fn insert_values(
    schema: &TableSchema,
    auto_increment: Option<AutoIncrement>,
    columns: Option<Vec<String>>,
    supplied: Vec<Value>,
) -> Result<ResolvedInsert> {
    let mut values = if let Some(columns) = columns {
        if columns.len() != supplied.len() {
            return Err(Error::Type(format!(
                "INSERT names {} columns but supplies {} values",
                columns.len(),
                supplied.len()
            )));
        }
        let mut seen = BTreeSet::new();
        let mut values = vec![Value::Null; schema.columns.len()];
        for (name, value) in columns.into_iter().zip(supplied) {
            if !seen.insert(name.clone()) {
                return Err(Error::Schema(format!("duplicate INSERT column {name:?}")));
            }
            let index = require_column(schema, &name)?;
            values[index] = value;
        }
        values
    } else {
        if supplied.len() != schema.columns.len() {
            return Err(Error::Type(format!(
                "table {:?} expects {} values, got {}",
                schema.name,
                schema.columns.len(),
                supplied.len()
            )));
        }
        supplied
    };

    let next_auto_increment = if let Some(auto_increment) = auto_increment {
        let value = values
            .get_mut(auto_increment.column)
            .expect("validated auto-increment column is in the schema");
        match value {
            Value::Null => {
                let next = auto_increment.last.checked_add(1).ok_or_else(|| {
                    Error::Constraint(format!(
                        "auto-increment sequence for table {:?} is exhausted",
                        schema.name
                    ))
                })?;
                *value = Value::Integer(next);
                Some(next)
            }
            Value::Integer(value) if *value > auto_increment.last => Some(*value),
            Value::Integer(_) | Value::Text(_) | Value::Boolean(_) => None,
        }
    } else {
        None
    };

    for (value, column) in values.iter().zip(&schema.columns) {
        validate_value(value, column)?;
    }
    Ok(ResolvedInsert {
        values,
        next_auto_increment,
    })
}

pub(crate) fn assignments(
    schema: &TableSchema,
    auto_increment: Option<AutoIncrement>,
    assignments: &[Assignment],
) -> Result<ResolvedAssignments> {
    let mut resolved = Vec::with_capacity(assignments.len());
    let mut seen = BTreeSet::new();
    for assignment in assignments {
        if !seen.insert(assignment.column.as_str()) {
            return Err(Error::Schema(format!(
                "duplicate UPDATE assignment for column {:?}",
                assignment.column
            )));
        }
        let index = require_column(schema, &assignment.column)?;
        validate_value(&assignment.value, &schema.columns[index])?;
        resolved.push((index, assignment.value.clone()));
    }
    let next_auto_increment = auto_increment.and_then(|auto_increment| {
        resolved
            .iter()
            .find(|(column, _)| *column == auto_increment.column)
            .and_then(|(_, value)| match value {
                Value::Integer(value) if *value > auto_increment.last => Some(*value),
                Value::Integer(_) | Value::Text(_) | Value::Boolean(_) | Value::Null => None,
            })
    });
    Ok(ResolvedAssignments {
        values: resolved,
        next_auto_increment,
    })
}

pub(crate) fn predicate<'a>(
    schema: &TableSchema,
    predicate: &'a Predicate,
) -> Result<ResolvedPredicate<'a>> {
    let column = require_column(schema, &predicate.column)?;
    let definition = &schema.columns[column];
    match &predicate.operator {
        PredicateOperator::Equal(Value::Null) | PredicateOperator::NotEqual(Value::Null) => {
            Err(Error::Type(String::from(
                "NULL cannot be compared with `=` or `!=`; use IS NULL or IS NOT NULL",
            )))
        }
        PredicateOperator::Equal(value) => {
            validate_value(value, definition)?;
            Ok(ResolvedPredicate::Equal { column, value })
        }
        PredicateOperator::NotEqual(value) => {
            validate_value(value, definition)?;
            Ok(ResolvedPredicate::NotEqual { column, value })
        }
        PredicateOperator::Like(pattern) => {
            if definition.data_type != DataType::Text {
                return Err(Error::Type(format!(
                    "LIKE requires a TEXT column; {:?} is {}",
                    definition.name, definition.data_type
                )));
            }
            Ok(ResolvedPredicate::Like { column, pattern })
        }
        PredicateOperator::IsNull => Ok(ResolvedPredicate::IsNull { column }),
        PredicateOperator::IsNotNull => Ok(ResolvedPredicate::IsNotNull { column }),
    }
}

fn require_column(schema: &TableSchema, name: &str) -> Result<usize> {
    schema
        .columns
        .iter()
        .position(|column| column.name == name)
        .ok_or_else(|| {
            Error::Schema(format!(
                "unknown column {name:?} in table {:?}",
                schema.name
            ))
        })
}

#[cfg(test)]
mod tests;
