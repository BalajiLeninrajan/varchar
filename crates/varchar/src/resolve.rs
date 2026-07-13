//! Schema-aware SQL name and type resolution.
//!
//! This layer turns parser-owned names into column positions and validates
//! logical values. It deliberately knows nothing about storage encodings,
//! regular expressions, row scans, or candidate commits.

use std::collections::BTreeSet;

use crate::sql::{
    Assignment, CreateTable, Predicate, PredicateOperator, Projection, TableConstraint,
};
use crate::storage::{Catalog, ForeignKey, TableSchema};
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

    let mut primary_key = None;
    let mut foreign_keys = Vec::new();
    let mut columns = Vec::with_capacity(statement.columns.len());

    for (index, column) in statement.columns.into_iter().enumerate() {
        if column.primary_key && primary_key.replace(index).is_some() {
            return Err(multiple_primary_keys(&statement.table));
        }
        if let Some(reference) = column.references {
            foreign_keys.push(ForeignKey {
                column: index,
                referenced_table: reference.table,
                referenced_column: reference.column,
            });
        }
        columns.push(Column {
            name: column.name,
            data_type: column.data_type,
            nullable: column.nullable && !column.primary_key,
        });
    }

    for constraint in statement.constraints {
        match constraint {
            TableConstraint::PrimaryKey(name) => {
                let index =
                    local_constraint_column(&columns, &statement.table, &name, "PRIMARY KEY")?;
                match primary_key {
                    Some(existing) if existing == index => {
                        return Err(Error::Schema(format!(
                            "duplicate PRIMARY KEY declaration for column {name:?}"
                        )));
                    }
                    Some(_) => return Err(multiple_primary_keys(&statement.table)),
                    None => primary_key = Some(index),
                }
                columns[index].nullable = false;
            }
            TableConstraint::ForeignKey { column, reference } => {
                let index =
                    local_constraint_column(&columns, &statement.table, &column, "FOREIGN KEY")?;
                if foreign_keys
                    .iter()
                    .any(|foreign_key: &ForeignKey| foreign_key.column == index)
                {
                    return Err(Error::Schema(format!(
                        "duplicate FOREIGN KEY declaration for column {column:?}"
                    )));
                }
                foreign_keys.push(ForeignKey {
                    column: index,
                    referenced_table: reference.table,
                    referenced_column: reference.column,
                });
            }
        }
    }

    Ok(TableSchema {
        name: statement.table,
        columns,
        primary_key,
        foreign_keys,
    })
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
    use super::{assignments, create_schema, insert_values, predicate};
    use crate::sql::{self, Assignment, Predicate, PredicateOperator, Statement};
    use crate::storage::{Catalog, ForeignKey, TableSchema};
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
            primary_key: None,
            foreign_keys: Vec::new(),
        }
    }

    fn create_table(sql: &str) -> crate::sql::CreateTable {
        let Statement::CreateTable(statement) = sql::parse(sql).expect("statement parses") else {
            panic!("expected CREATE TABLE");
        };
        statement
    }

    #[test]
    fn create_schema_normalizes_inline_and_table_key_metadata() {
        for sql in [
            "CREATE TABLE children (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id))",
            "CREATE TABLE children (id INTEGER, parent_id INTEGER, PRIMARY KEY (id), FOREIGN KEY (parent_id) REFERENCES parents(id))",
        ] {
            let schema =
                create_schema(&Catalog::empty(), create_table(sql)).expect("schema resolves");
            assert_eq!(schema.primary_key, Some(0));
            assert!(!schema.columns[0].nullable);
            assert_eq!(
                schema.foreign_keys,
                vec![ForeignKey {
                    column: 1,
                    referenced_table: String::from("parents"),
                    referenced_column: String::from("id"),
                }]
            );
        }
    }

    #[test]
    fn create_schema_owns_table_constraint_policy() {
        for (sql, expected) in [
            (
                "CREATE TABLE items (id INTEGER, PRIMARY KEY (missing))",
                "PRIMARY KEY references unknown column \"missing\" in table \"items\"",
            ),
            (
                "CREATE TABLE items (id INTEGER, FOREIGN KEY (missing) REFERENCES parents(id))",
                "FOREIGN KEY references unknown column \"missing\" in table \"items\"",
            ),
            (
                "CREATE TABLE items (id INTEGER PRIMARY KEY, PRIMARY KEY (id))",
                "duplicate PRIMARY KEY declaration for column \"id\"",
            ),
            (
                "CREATE TABLE items (id INTEGER PRIMARY KEY, other INTEGER, PRIMARY KEY (other))",
                "table \"items\" may have only one PRIMARY KEY column",
            ),
            (
                "CREATE TABLE items (id INTEGER, parent_id INTEGER REFERENCES parents(id), FOREIGN KEY (parent_id) REFERENCES parents(id))",
                "duplicate FOREIGN KEY declaration for column \"parent_id\"",
            ),
        ] {
            assert!(matches!(
                create_schema(&Catalog::empty(), create_table(sql)),
                Err(Error::Schema(ref message)) if message == expected
            ));
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
