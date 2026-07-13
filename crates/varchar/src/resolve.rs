//! Schema-aware SQL name and type resolution.
//!
//! This layer turns parser-owned names into column positions and validates
//! logical values. It deliberately knows nothing about storage encodings,
//! regular expressions, row scans, or candidate commits.

use std::collections::BTreeSet;

use crate::sql::{Assignment, CreateTable, Predicate, PredicateOperator, Projection};
use crate::storage::{Catalog, TableSchema};
use crate::value::validate_value;
use crate::{Column, DataType, Error, Result, Value};

pub(crate) enum ResolvedPredicate<'a> {
    Equal { column: usize, value: &'a Value },
    NotEqual { column: usize, value: &'a Value },
    Like { column: usize, pattern: &'a str },
    IsNull { column: usize },
    IsNotNull { column: usize },
}

pub(crate) fn create_schema(catalog: &Catalog, statement: CreateTable) -> Result<TableSchema> {
    if catalog.table(&statement.table).is_some() {
        return Err(Error::Schema(format!(
            "table {:?} already exists",
            statement.table
        )));
    }

    Ok(TableSchema {
        name: statement.table,
        columns: statement
            .columns
            .into_iter()
            .map(|column| Column {
                name: column.name,
                data_type: column.data_type,
                nullable: column.nullable,
            })
            .collect(),
    })
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
    columns: Option<Vec<String>>,
    supplied: Vec<Value>,
) -> Result<Vec<Value>> {
    let values = if let Some(columns) = columns {
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

    for (value, column) in values.iter().zip(&schema.columns) {
        validate_value(value, column)?;
    }
    Ok(values)
}

pub(crate) fn assignments(
    schema: &TableSchema,
    assignments: &[Assignment],
) -> Result<Vec<(usize, Value)>> {
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
    Ok(resolved)
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
mod tests {
    use super::{assignments, insert_values, predicate};
    use crate::sql::{Assignment, Predicate, PredicateOperator};
    use crate::storage::TableSchema;
    use crate::{Column, DataType, Error, Value};

    fn people_schema() -> TableSchema {
        TableSchema {
            name: String::from("people"),
            columns: vec![
                Column {
                    name: String::from("id"),
                    data_type: DataType::Integer,
                    nullable: false,
                },
                Column {
                    name: String::from("note"),
                    data_type: DataType::Text,
                    nullable: true,
                },
                Column {
                    name: String::from("active"),
                    data_type: DataType::Boolean,
                    nullable: false,
                },
            ],
        }
    }

    #[test]
    fn named_insert_resolves_names_before_validating_the_row() {
        let schema = people_schema();
        assert!(matches!(
            insert_values(
                &schema,
                Some(vec![String::from("id"), String::from("missing")]),
                vec![Value::Text(String::from("wrong")), Value::Integer(1)],
            ),
            Err(Error::Schema(ref message))
                if message == "unknown column \"missing\" in table \"people\""
        ));

        assert_eq!(
            insert_values(
                &schema,
                Some(vec![
                    String::from("active"),
                    String::from("id"),
                    String::from("note"),
                ]),
                vec![
                    Value::Boolean(true),
                    Value::Integer(7),
                    Value::Text(String::from("ready")),
                ],
            )
            .expect("named values resolve"),
            vec![
                Value::Integer(7),
                Value::Text(String::from("ready")),
                Value::Boolean(true),
            ]
        );
    }

    #[test]
    fn assignments_validate_in_statement_order() {
        let schema = people_schema();
        let assignments_to_resolve = vec![
            Assignment {
                column: String::from("id"),
                value: Value::Text(String::from("wrong")),
            },
            Assignment {
                column: String::from("missing"),
                value: Value::Integer(1),
            },
        ];
        assert!(matches!(
            assignments(&schema, &assignments_to_resolve),
            Err(Error::Type(ref message))
                if message == "column \"id\" expects INTEGER, got TEXT"
        ));
    }

    #[test]
    fn duplicate_insert_columns_and_assignments_are_rejected() {
        let schema = people_schema();
        assert!(matches!(
            insert_values(
                &schema,
                Some(vec![String::from("id"), String::from("id")]),
                vec![Value::Integer(1), Value::Integer(2)],
            ),
            Err(Error::Schema(ref message)) if message == "duplicate INSERT column \"id\""
        ));

        let duplicate_assignments = vec![
            Assignment {
                column: String::from("id"),
                value: Value::Integer(1),
            },
            Assignment {
                column: String::from("id"),
                value: Value::Integer(2),
            },
        ];
        assert!(matches!(
            assignments(&schema, &duplicate_assignments),
            Err(Error::Schema(ref message))
                if message == "duplicate UPDATE assignment for column \"id\""
        ));
    }

    #[test]
    fn predicate_resolution_preserves_name_and_operator_error_order() {
        let schema = people_schema();
        let missing = Predicate {
            column: String::from("missing"),
            operator: PredicateOperator::Equal(Value::Null),
        };
        assert!(matches!(
            predicate(&schema, &missing),
            Err(Error::Schema(ref message))
                if message == "unknown column \"missing\" in table \"people\""
        ));

        let null_comparison = Predicate {
            column: String::from("id"),
            operator: PredicateOperator::Equal(Value::Null),
        };
        assert!(matches!(
            predicate(&schema, &null_comparison),
            Err(Error::Type(ref message))
                if message
                    == "NULL cannot be compared with `=` or `!=`; use IS NULL or IS NOT NULL"
        ));

        let wrong_like_type = Predicate {
            column: String::from("id"),
            operator: PredicateOperator::Like(String::from("anything")),
        };
        assert!(matches!(
            predicate(&schema, &wrong_like_type),
            Err(Error::Type(ref message))
                if message == "LIKE requires a TEXT column; \"id\" is INTEGER"
        ));
    }
}
