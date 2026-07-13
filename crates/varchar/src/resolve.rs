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
    let mut saw_not_null = vec![false; columns.len()];
    let mut saw_foreign_key = vec![false; columns.len()];
    let mut column_index = 0;

    // Fold local declarations in source order. Cross-table checks wait until
    // the complete local primary key is available for self references.
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
    Ok(schema)
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
    use crate::storage::{Catalog, ForeignKey, TableSchema, validate_and_catalog};
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

    fn keyed_parent_catalog() -> Catalog {
        validate_and_catalog("V2;~S|parents|id:I:!|code:I:?|label:T:?;~P|parents|id;")
            .expect("parent catalog is valid")
    }

    #[test]
    fn create_schema_normalizes_inline_and_table_key_metadata() {
        for sql in [
            "CREATE TABLE children (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parents(id))",
            "CREATE TABLE children (id INTEGER, parent_id INTEGER, PRIMARY KEY (id), FOREIGN KEY (parent_id) REFERENCES parents(id))",
            "CREATE TABLE children (PRIMARY KEY (id), FOREIGN KEY (parent_id) REFERENCES parents(id), id INTEGER, parent_id INTEGER)",
        ] {
            let schema =
                create_schema(&keyed_parent_catalog(), create_table(sql)).expect("schema resolves");
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
    fn create_schema_owns_column_shape_and_modifier_policy() {
        for (sql, expected) in [
            (
                "CREATE TABLE items (missing INTEGER, id INTEGER, id TEXT)",
                "duplicate column name \"id\"",
            ),
            (
                "CREATE TABLE items (id INTEGER NOT NULL NOT NULL)",
                "duplicate NOT NULL declaration for column \"id\"",
            ),
            (
                "CREATE TABLE items (id INTEGER PRIMARY KEY PRIMARY KEY)",
                "duplicate PRIMARY KEY declaration for column \"id\"",
            ),
            (
                "CREATE TABLE items (id INTEGER REFERENCES parents(id) REFERENCES parents(id))",
                "duplicate REFERENCES declaration for column \"id\"",
            ),
            (
                "CREATE TABLE items (PRIMARY KEY (missing))",
                "table must contain at least one column",
            ),
        ] {
            assert!(matches!(
                create_schema(&keyed_parent_catalog(), create_table(sql)),
                Err(Error::Schema(ref message)) if message == expected
            ));
        }
    }

    #[test]
    fn duplicate_columns_precede_declaration_errors_but_declarations_keep_source_order() {
        let duplicate_column =
            create_table("CREATE TABLE items (PRIMARY KEY (missing), id INTEGER, id INTEGER)");
        assert!(matches!(
            create_schema(&Catalog::empty(), duplicate_column),
            Err(Error::Schema(ref message)) if message == "duplicate column name \"id\""
        ));

        let declarations = create_table(
            "CREATE TABLE items (id INTEGER NOT NULL NOT NULL PRIMARY KEY PRIMARY KEY)",
        );
        assert!(matches!(
            create_schema(&Catalog::empty(), declarations),
            Err(Error::Schema(ref message))
                if message == "duplicate NOT NULL declaration for column \"id\""
        ));

        let interleaved = create_table(
            "CREATE TABLE items (FOREIGN KEY (missing) REFERENCES parents(id), id INTEGER NOT NULL NOT NULL)",
        );
        assert!(matches!(
            create_schema(&keyed_parent_catalog(), interleaved),
            Err(Error::Schema(ref message))
                if message == "FOREIGN KEY references unknown column \"missing\" in table \"items\""
        ));
    }

    #[test]
    fn create_schema_resolves_foreign_key_targets_before_storage() {
        for (sql, expected) in [
            (
                "CREATE TABLE children (parent_id INTEGER REFERENCES missing(id))",
                "foreign key references unknown or later table \"missing\"",
            ),
            (
                "CREATE TABLE children (parent_id INTEGER REFERENCES parents(missing))",
                "foreign key target \"parents\".\"missing\" is not its table's primary key",
            ),
            (
                "CREATE TABLE children (parent_id INTEGER REFERENCES parents(code))",
                "foreign key target \"parents\".\"code\" is not its table's primary key",
            ),
            (
                "CREATE TABLE children (parent_id TEXT REFERENCES parents(id))",
                "foreign-key columns \"children\".\"parent_id\" and \"parents\".\"id\" have different types",
            ),
        ] {
            assert!(matches!(
                create_schema(&keyed_parent_catalog(), create_table(sql)),
                Err(Error::Schema(ref message)) if message == expected
            ));
        }

        let source_order = create_table(
            "CREATE TABLE children (first INTEGER REFERENCES missing_first(id), second INTEGER REFERENCES missing_second(id))",
        );
        assert!(matches!(
            create_schema(&Catalog::empty(), source_order),
            Err(Error::Schema(ref message))
                if message == "foreign key references unknown or later table \"missing_first\""
        ));
    }

    #[test]
    fn self_referential_foreign_keys_use_the_finished_local_primary_key() {
        let schema = create_schema(
            &Catalog::empty(),
            create_table(
                "CREATE TABLE nodes (parent_id INTEGER REFERENCES nodes(id), id INTEGER, PRIMARY KEY (id))",
            ),
        )
        .expect("self reference resolves against the final local schema");

        assert_eq!(schema.primary_key, Some(1));
        assert_eq!(
            schema.foreign_keys,
            vec![ForeignKey {
                column: 0,
                referenced_table: String::from("nodes"),
                referenced_column: String::from("id"),
            }]
        );
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
