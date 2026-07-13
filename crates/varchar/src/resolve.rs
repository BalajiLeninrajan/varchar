//! Schema-aware SQL name and type resolution.
//!
//! This layer turns parser-owned names into column positions and validates
//! logical values. It deliberately knows nothing about storage encodings,
//! regular expressions, row scans, or candidate commits.

use std::collections::BTreeSet;

use crate::limits::check_limit;
use crate::sql::{
    Assignment, ColumnModifier, ColumnRef, CreateElement, CreateTable, Predicate,
    PredicateOperator, Projection, ProjectionItem, Select, TableConstraint,
};
use crate::storage::{AutoIncrement, Catalog, ForeignKey, TableSchema};
use crate::value::validate_value;
use crate::{Column, DataType, Error, Result, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ResolvedPredicate<'a> {
    Equal { column: usize, value: &'a Value },
    NotEqual { column: usize, value: &'a Value },
    Like { column: usize, atoms: Vec<LikeAtom> },
    IsNull { column: usize },
    IsNotNull { column: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LikeAtom {
    AnySequence,
    AnyScalar,
    Literal(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ColumnLocation {
    pub(crate) source: usize,
    pub(crate) column: usize,
}

pub(crate) struct ResolvedJoinCondition {
    pub(crate) left: ColumnLocation,
    pub(crate) right: ColumnLocation,
}

pub(crate) struct ResolvedJoin {
    pub(crate) source: usize,
    pub(crate) conditions: Vec<ResolvedJoinCondition>,
}

pub(crate) struct ResolvedSourcePredicate<'a> {
    pub(crate) source: usize,
    pub(crate) predicate: ResolvedPredicate<'a>,
}

pub(crate) struct ResolvedSelect<'catalog, 'statement> {
    pub(crate) sources: Vec<&'catalog TableSchema>,
    pub(crate) projection: Vec<ColumnLocation>,
    pub(crate) joins: Vec<ResolvedJoin>,
    pub(crate) predicates: Vec<ResolvedSourcePredicate<'statement>>,
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

pub(crate) fn select<'catalog, 'statement>(
    catalog: &'catalog Catalog,
    statement: &'statement Select,
    max_join_sources: usize,
    max_predicates: usize,
) -> Result<ResolvedSelect<'catalog, 'statement>> {
    let source_count = statement
        .joins
        .len()
        .checked_add(1)
        .ok_or(Error::ResourceLimit {
            resource: "JOIN sources",
            limit: max_join_sources,
        })?;
    check_limit(source_count, max_join_sources, "JOIN sources")?;

    let mut sources = Vec::with_capacity(source_count);
    sources.push(require_table(catalog, &statement.table)?);
    for join in &statement.joins {
        if sources.iter().any(|schema| schema.name == join.table) {
            return Err(Error::Schema(format!(
                "table {:?} appears more than once in a SELECT",
                join.table
            )));
        }
        sources.push(require_table(catalog, &join.table)?);
    }

    let projection = resolve_projection(&sources, &statement.projection)?;
    let joins = resolve_joins(statement, &sources)?;
    check_limit(
        statement.predicates.len(),
        max_predicates,
        "WHERE predicates",
    )?;
    let predicates = statement
        .predicates
        .iter()
        .map(|predicate| resolve_select_predicate(&sources, predicate))
        .collect::<Result<Vec<_>>>()?;

    Ok(ResolvedSelect {
        sources,
        projection,
        joins,
        predicates,
    })
}

fn resolve_select_predicate<'a>(
    sources: &[&TableSchema],
    predicate: &'a Predicate,
) -> Result<ResolvedSourcePredicate<'a>> {
    let location = resolve_column(sources, &predicate.column)?;
    Ok(ResolvedSourcePredicate {
        source: location.source,
        predicate: predicate_at(
            sources[location.source],
            location.column,
            &predicate.operator,
        )?,
    })
}

fn resolve_projection(
    schemas: &[&TableSchema],
    projection: &Projection,
) -> Result<Vec<ColumnLocation>> {
    match projection {
        Projection::All => Ok(schemas
            .iter()
            .enumerate()
            .flat_map(|(source, schema)| {
                (0..schema.columns.len()).map(move |column| ColumnLocation { source, column })
            })
            .collect()),
        Projection::Items(items) => {
            let mut resolved = Vec::new();
            for item in items {
                match item {
                    ProjectionItem::Column(column) => {
                        resolved.push(resolve_column(schemas, column)?);
                    }
                    ProjectionItem::QualifiedAll(table) => {
                        let source = schemas
                            .iter()
                            .position(|schema| schema.name == *table)
                            .ok_or_else(|| {
                                Error::Schema(format!(
                                    "unknown table qualifier {table:?} in projection"
                                ))
                            })?;
                        resolved.extend(
                            (0..schemas[source].columns.len())
                                .map(|column| ColumnLocation { source, column }),
                        );
                    }
                }
            }
            Ok(resolved)
        }
    }
}

fn resolve_joins(statement: &Select, schemas: &[&TableSchema]) -> Result<Vec<ResolvedJoin>> {
    let mut joins = Vec::with_capacity(statement.joins.len());
    for (join_index, join) in statement.joins.iter().enumerate() {
        let source = join_index + 1;
        let visible = &schemas[..=source];
        let mut conditions = Vec::with_capacity(join.conditions.len());
        let mut connects_source = false;
        for condition in &join.conditions {
            let left = resolve_column(visible, &condition.left)?;
            let right = resolve_column(visible, &condition.right)?;
            connects_source |= (left.source == source && right.source < source)
                || (right.source == source && left.source < source);
            let left_type = schemas[left.source].columns[left.column].data_type;
            let right_type = schemas[right.source].columns[right.column].data_type;
            if left_type != right_type {
                return Err(Error::Type(format!(
                    "JOIN columns {:?}.{:?} and {:?}.{:?} have different types",
                    schemas[left.source].name,
                    schemas[left.source].columns[left.column].name,
                    schemas[right.source].name,
                    schemas[right.source].columns[right.column].name
                )));
            }
            conditions.push(ResolvedJoinCondition { left, right });
        }
        if !connects_source {
            return Err(Error::Schema(format!(
                "JOIN for table {:?} must connect it to an earlier table",
                join.table
            )));
        }
        joins.push(ResolvedJoin { source, conditions });
    }
    Ok(joins)
}

fn resolve_column(schemas: &[&TableSchema], reference: &ColumnRef) -> Result<ColumnLocation> {
    if let Some(qualifier) = &reference.qualifier {
        let source = schemas
            .iter()
            .position(|schema| schema.name == *qualifier)
            .ok_or_else(|| Error::Schema(format!("unknown table qualifier {qualifier:?}")))?;
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
                return Err(Error::Schema(format!(
                    "ambiguous column {:?}; qualify it with a table name",
                    reference.name
                )));
            }
            match_ = Some(ColumnLocation { source, column });
        }
    }
    match_.ok_or_else(|| {
        if let [schema] = schemas {
            Error::Schema(format!(
                "unknown column {:?} in table {:?}",
                reference.name, schema.name
            ))
        } else {
            Error::Schema(format!("unknown column {:?}", reference.name))
        }
    })
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
    let column = require_local_column(schema, &predicate.column)?;
    predicate_at(schema, column, &predicate.operator)
}

fn predicate_at<'a>(
    schema: &TableSchema,
    column: usize,
    operator: &'a PredicateOperator,
) -> Result<ResolvedPredicate<'a>> {
    let definition = &schema.columns[column];
    match operator {
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
            Ok(ResolvedPredicate::Like {
                column,
                atoms: resolve_like_pattern(pattern)?,
            })
        }
        PredicateOperator::IsNull => Ok(ResolvedPredicate::IsNull { column }),
        PredicateOperator::IsNotNull => Ok(ResolvedPredicate::IsNotNull { column }),
    }
}

fn resolve_like_pattern(pattern: &str) -> Result<Vec<LikeAtom>> {
    let mut atoms = Vec::new();
    let mut characters = pattern.chars();
    while let Some(character) = characters.next() {
        match character {
            '%' => atoms.push(LikeAtom::AnySequence),
            '_' => atoms.push(LikeAtom::AnyScalar),
            '\\' => {
                let Some(escaped) = characters.next() else {
                    return Err(Error::Type(String::from(
                        "LIKE pattern ends with an incomplete escape",
                    )));
                };
                if !matches!(escaped, '%' | '_' | '\\') {
                    return Err(Error::Type(format!(
                        "LIKE pattern contains unsupported escape \\{escaped}"
                    )));
                }
                atoms.push(LikeAtom::Literal(escaped));
            }
            literal => atoms.push(LikeAtom::Literal(literal)),
        }
    }
    Ok(atoms)
}

fn require_local_column(schema: &TableSchema, reference: &ColumnRef) -> Result<usize> {
    if let Some(qualifier) = &reference.qualifier
        && qualifier != &schema.name
    {
        return Err(Error::Schema(format!(
            "unknown table qualifier {qualifier:?} for table {:?}",
            schema.name
        )));
    }
    require_column(schema, &reference.name)
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
    use super::{
        LikeAtom, ResolvedPredicate, assignments, create_schema, insert_values, predicate, select,
    };
    use crate::sql::{self, Assignment, ColumnRef, Predicate, PredicateOperator, Statement};
    use crate::storage::{
        self, AutoIncrement, Catalog, ForeignKey, TableSchema, validate_and_catalog,
    };
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
            let resolved =
                create_schema(&keyed_parent_catalog(), create_table(sql)).expect("schema resolves");
            assert_eq!(resolved.auto_increment, None);
            let schema = resolved.schema;
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
        let resolved = create_schema(
            &Catalog::empty(),
            create_table(
                "CREATE TABLE nodes (parent_id INTEGER REFERENCES nodes(id), id INTEGER, PRIMARY KEY (id))",
            ),
        )
        .expect("self reference resolves against the final local schema");
        assert_eq!(resolved.auto_increment, None);
        let schema = resolved.schema;

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
    fn auto_increment_uses_the_finished_primary_key() {
        for sql in [
            "CREATE TABLE ids (id INTEGER AUTOINCREMENT PRIMARY KEY)",
            "CREATE TABLE ids (id INTEGER AUTOINCREMENT, PRIMARY KEY (id))",
            "CREATE TABLE ids (PRIMARY KEY (id), id INTEGER AUTO_INCREMENT)",
        ] {
            let resolved =
                create_schema(&Catalog::empty(), create_table(sql)).expect("schema resolves");
            assert_eq!(resolved.auto_increment, Some(0));
            assert_eq!(resolved.schema.primary_key, Some(0));
            assert!(!resolved.schema.columns[0].nullable);
        }
    }

    #[test]
    fn auto_increment_duplicates_and_applicability_are_resolver_owned() {
        for (sql, expected) in [
            (
                "CREATE TABLE ids (id INTEGER PRIMARY KEY AUTOINCREMENT AUTO_INCREMENT)",
                "duplicate AUTOINCREMENT declaration for column \"id\"",
            ),
            (
                "CREATE TABLE ids (a INTEGER PRIMARY KEY AUTOINCREMENT, b INTEGER AUTOINCREMENT)",
                "table \"ids\" may have only one auto-increment column",
            ),
            (
                "CREATE TABLE ids (id TEXT PRIMARY KEY AUTOINCREMENT)",
                "auto-increment column \"ids\".\"id\" must be its INTEGER primary key",
            ),
            (
                "CREATE TABLE ids (id INTEGER AUTOINCREMENT)",
                "auto-increment column \"ids\".\"id\" must be its INTEGER primary key",
            ),
        ] {
            assert!(matches!(
                create_schema(&Catalog::empty(), create_table(sql)),
                Err(Error::Schema(ref message)) if message == expected
            ));
        }
    }

    #[test]
    fn auto_increment_declaration_and_applicability_errors_have_stable_precedence() {
        let duplicate_auto = create_table(
            "CREATE TABLE ids (id INTEGER AUTOINCREMENT AUTO_INCREMENT PRIMARY KEY PRIMARY KEY)",
        );
        assert!(matches!(
            create_schema(&Catalog::empty(), duplicate_auto),
            Err(Error::Schema(ref message))
                if message == "duplicate AUTOINCREMENT declaration for column \"id\""
        ));

        let duplicate_primary = create_table(
            "CREATE TABLE ids (id INTEGER PRIMARY KEY PRIMARY KEY AUTOINCREMENT AUTO_INCREMENT)",
        );
        assert!(matches!(
            create_schema(&Catalog::empty(), duplicate_primary),
            Err(Error::Schema(ref message))
                if message == "duplicate PRIMARY KEY declaration for column \"id\""
        ));

        let invalid_foreign_key_before_applicability = create_table(
            "CREATE TABLE ids (id TEXT PRIMARY KEY AUTOINCREMENT, parent TEXT REFERENCES missing(id))",
        );
        assert!(matches!(
            create_schema(&Catalog::empty(), invalid_foreign_key_before_applicability),
            Err(Error::Schema(ref message))
                if message == "foreign key references unknown or later table \"missing\""
        ));
    }

    #[test]
    fn auto_increment_resolution_generates_and_tracks_only_new_high_water_marks() {
        let schema = TableSchema {
            name: String::from("ids"),
            columns: vec![Column {
                name: String::from("id"),
                data_type: DataType::Integer,
                nullable: false,
            }],
            primary_key: Some(0),
            foreign_keys: Vec::new(),
        };
        let auto_increment = Some(AutoIncrement { column: 0, last: 4 });

        let generated = insert_values(&schema, auto_increment, None, vec![Value::Null])
            .expect("NULL generates a value");
        assert_eq!(generated.values, vec![Value::Integer(5)]);
        assert_eq!(generated.next_auto_increment, Some(5));

        let explicit_lower = insert_values(&schema, auto_increment, None, vec![Value::Integer(-1)])
            .expect("an explicit lower value is retained");
        assert_eq!(explicit_lower.values, vec![Value::Integer(-1)]);
        assert_eq!(explicit_lower.next_auto_increment, None);
    }

    #[test]
    fn sequence_exhaustion_precedes_remaining_value_validation() {
        let schema = TableSchema {
            name: String::from("ids"),
            columns: vec![
                Column {
                    name: String::from("id"),
                    data_type: DataType::Integer,
                    nullable: false,
                },
                Column {
                    name: String::from("required"),
                    data_type: DataType::Text,
                    nullable: false,
                },
            ],
            primary_key: Some(0),
            foreign_keys: Vec::new(),
        };

        assert!(matches!(
            insert_values(
                &schema,
                Some(AutoIncrement {
                    column: 0,
                    last: i64::MAX,
                }),
                None,
                vec![Value::Null, Value::Null],
            ),
            Err(Error::Constraint(ref message))
                if message == "auto-increment sequence for table \"ids\" is exhausted"
        ));
    }

    #[test]
    fn joined_select_resolution_tracks_sources_locations_and_predicates() {
        let catalog = storage::validate_and_catalog(
            "V2;~S|authors|id:I:!|name:T:!;~S|books|author_id:I:!|title:T:!;",
        )
        .expect("fixture catalog is valid");
        let Statement::Select(statement) = sql::parse(
            "SELECT authors.name, books.title FROM authors \
             JOIN books ON authors.id = books.author_id \
             WHERE books.title LIKE 'N%' AND authors.name = 'Ada'",
        )
        .expect("statement parses") else {
            panic!("expected SELECT");
        };

        let resolved = select(&catalog, &statement, 4, 4).expect("SELECT resolves");
        assert_eq!(resolved.sources[0].name, "authors");
        assert_eq!(resolved.sources[1].name, "books");
        assert_eq!(
            (resolved.projection[0].source, resolved.projection[0].column),
            (0, 1)
        );
        assert_eq!(
            (resolved.projection[1].source, resolved.projection[1].column),
            (1, 1)
        );
        assert_eq!(resolved.joins[0].source, 1);
        assert_eq!(resolved.joins[0].conditions[0].left.source, 0);
        assert_eq!(resolved.joins[0].conditions[0].right.source, 1);
        let first = &resolved.predicates[0];
        assert_eq!(first.source, 1);
        assert!(matches!(
            &first.predicate,
            ResolvedPredicate::Like {
                column: 1,
                atoms
            } if atoms == &[LikeAtom::Literal('N'), LikeAtom::AnySequence]
        ));
        let second = &resolved.predicates[1];
        assert_eq!(second.source, 0);
        assert!(matches!(
            &second.predicate,
            ResolvedPredicate::Equal {
                column: 1,
                value: Value::Text(value)
            } if value == "Ada"
        ));
    }

    #[test]
    fn repeated_select_sources_are_rejected_during_resolution() {
        let catalog = storage::validate_and_catalog("V2;~S|nodes|id:I:!|parent_id:I:?;")
            .expect("fixture catalog is valid");
        let Statement::Select(statement) =
            sql::parse("SELECT nodes.id FROM nodes JOIN nodes ON nodes.parent_id = nodes.id")
                .expect("repeated sources are valid syntax")
        else {
            panic!("expected SELECT");
        };

        assert!(matches!(
            select(&catalog, &statement, 4, 4),
            Err(Error::Schema(ref message))
                if message == "table \"nodes\" appears more than once in a SELECT"
        ));
    }

    #[test]
    fn select_predicates_resolve_in_statement_order() {
        let catalog = storage::validate_and_catalog("V2;~S|t|id:I:!|note:T:!;")
            .expect("fixture catalog is valid");
        let Statement::Select(invalid_like_first) =
            sql::parse(r"SELECT id FROM t WHERE note LIKE 'bad\q' AND missing = 1")
                .expect("statement parses")
        else {
            panic!("expected SELECT");
        };
        assert!(matches!(
            select(&catalog, &invalid_like_first, 4, 4),
            Err(Error::Type(ref message))
                if message == "LIKE pattern contains unsupported escape \\q"
        ));

        let Statement::Select(missing_first) =
            sql::parse(r"SELECT id FROM t WHERE missing = 1 AND note LIKE 'bad\q'")
                .expect("statement parses")
        else {
            panic!("expected SELECT");
        };
        assert!(matches!(
            select(&catalog, &missing_first, 4, 4),
            Err(Error::Schema(ref message))
                if message == "unknown column \"missing\" in table \"t\""
        ));
    }

    #[test]
    fn named_insert_resolves_names_before_validating_the_row() {
        let schema = people_schema();
        assert!(matches!(
            insert_values(
                &schema,
                None,
                Some(vec![String::from("id"), String::from("missing")]),
                vec![Value::Text(String::from("wrong")), Value::Integer(1)],
            ),
            Err(Error::Schema(ref message))
                if message == "unknown column \"missing\" in table \"people\""
        ));

        assert_eq!(
            insert_values(
                &schema,
                None,
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
            .expect("named values resolve")
            .values,
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
            assignments(&schema, None, &assignments_to_resolve),
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
                None,
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
            assignments(&schema, None, &duplicate_assignments),
            Err(Error::Schema(ref message))
                if message == "duplicate UPDATE assignment for column \"id\""
        ));
    }

    #[test]
    fn predicate_resolution_preserves_name_and_operator_error_order() {
        let schema = people_schema();
        let missing = Predicate {
            column: ColumnRef {
                qualifier: None,
                name: String::from("missing"),
            },
            operator: PredicateOperator::Equal(Value::Null),
        };
        assert!(matches!(
            predicate(&schema, &missing),
            Err(Error::Schema(ref message))
                if message == "unknown column \"missing\" in table \"people\""
        ));

        let null_comparison = Predicate {
            column: ColumnRef {
                qualifier: None,
                name: String::from("id"),
            },
            operator: PredicateOperator::Equal(Value::Null),
        };
        assert!(matches!(
            predicate(&schema, &null_comparison),
            Err(Error::Type(ref message))
                if message
                    == "NULL cannot be compared with `=` or `!=`; use IS NULL or IS NOT NULL"
        ));

        let wrong_like_type = Predicate {
            column: ColumnRef {
                qualifier: None,
                name: String::from("id"),
            },
            operator: PredicateOperator::Like(String::from("anything")),
        };
        assert!(matches!(
            predicate(&schema, &wrong_like_type),
            Err(Error::Type(ref message))
                if message == "LIKE requires a TEXT column; \"id\" is INTEGER"
        ));
    }
}
