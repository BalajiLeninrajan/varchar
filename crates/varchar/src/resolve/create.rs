//! `CREATE TABLE` resolution and validated schema assembly.

mod auto_increment;
mod foreign_key;
mod primary_key;
mod unique;

use std::collections::BTreeMap;

use auto_increment::{declare_auto_increment, validate_auto_increment};
use foreign_key::{declare_foreign_key, validate_foreign_key};
use primary_key::declare_primary_key;
use unique::declare_unique;

use crate::sql::{ColumnModifier, CreateElement, CreateTable, TableConstraint};
use crate::storage::{Catalog, TableSchema};
use crate::value::validate_value;
use crate::{Error, Result, SchemaColumn, Value};

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
    let mut column_indices = BTreeMap::new();
    for element in &elements {
        let CreateElement::Column(column) = element else {
            continue;
        };
        if column_indices
            .insert(column.name.clone(), columns.len())
            .is_some()
        {
            return Err(Error::Schema(format!(
                "duplicate column name {:?}",
                column.name
            )));
        }
        columns.push(SchemaColumn {
            name: column.name.clone(),
            data_type: column.data_type,
            nullable: true,
            default: None,
        });
    }
    if columns.is_empty() {
        return Err(Error::Schema(String::from(
            "table must contain at least one column",
        )));
    }

    let mut primary_key = None;
    let mut unique_columns = Vec::new();
    let mut foreign_keys = Vec::new();
    let mut foreign_key_orders = Vec::new();
    let mut auto_increment = None;
    let mut auto_increment_order = None;
    let mut default_orders = vec![None; columns.len()];
    let mut saw_not_null = vec![false; columns.len()];
    let mut saw_unique = vec![false; columns.len()];
    let mut saw_foreign_key = vec![false; columns.len()];
    let mut column_index = 0;
    let mut declaration_order = 0;

    // Fold local declarations in source order. Cross-table and AUTO checks
    // wait until the complete local primary key is available.
    for element in elements {
        match element {
            CreateElement::Column(column) => {
                let index = column_index;
                column_index += 1;
                for modifier in column.modifiers {
                    let order = declaration_order;
                    declaration_order += 1;
                    match modifier {
                        ColumnModifier::NotNull => {
                            if saw_not_null[index] {
                                validate_defaults_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    &default_orders,
                                    order,
                                )?;
                                return Err(Error::Schema(format!(
                                    "duplicate NOT NULL declaration for column {:?}",
                                    column.name
                                )));
                            }
                            saw_not_null[index] = true;
                            columns[index].nullable = false;
                        }
                        ColumnModifier::PrimaryKey => {
                            if let Err(error) = declare_primary_key(
                                &table,
                                &column.name,
                                index,
                                &mut primary_key,
                                &mut columns,
                            ) {
                                validate_defaults_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    &default_orders,
                                    order,
                                )?;
                                return Err(error);
                            }
                        }
                        ColumnModifier::Unique => {
                            if let Err(error) = declare_unique(
                                &column.name,
                                index,
                                &mut saw_unique,
                                &mut unique_columns,
                            ) {
                                validate_defaults_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    &default_orders,
                                    order,
                                )?;
                                return Err(error);
                            }
                        }
                        ColumnModifier::References(reference) => {
                            if let Err(error) = declare_foreign_key(
                                &column.name,
                                "REFERENCES",
                                index,
                                reference.table,
                                reference.column,
                                &mut saw_foreign_key,
                                &mut foreign_keys,
                            ) {
                                validate_defaults_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    &default_orders,
                                    order,
                                )?;
                                return Err(error);
                            }
                            foreign_key_orders.push(order);
                        }
                        ColumnModifier::AutoIncrement => {
                            if let Err(error) = declare_auto_increment(
                                &table,
                                &column.name,
                                index,
                                &mut auto_increment,
                            ) {
                                validate_defaults_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    &default_orders,
                                    order,
                                )?;
                                return Err(error);
                            }
                            auto_increment_order = Some(order);
                        }
                        ColumnModifier::Default(value) => {
                            if columns[index].default.is_some() {
                                validate_defaults_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    &default_orders,
                                    order,
                                )?;
                                return Err(Error::Schema(format!(
                                    "duplicate DEFAULT declaration for column {:?}",
                                    column.name
                                )));
                            }
                            columns[index].default = Some(value);
                            default_orders[index] = Some(order);
                        }
                    }
                }
            }
            CreateElement::Constraint(constraint) => {
                let order = declaration_order;
                declaration_order += 1;
                match constraint {
                    TableConstraint::PrimaryKey(name) => {
                        let index = match local_constraint_column(
                            &column_indices,
                            &table,
                            &name,
                            "PRIMARY KEY",
                        ) {
                            Ok(index) => index,
                            Err(error) => {
                                validate_defaults_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    &default_orders,
                                    order,
                                )?;
                                return Err(error);
                            }
                        };
                        if let Err(error) = declare_primary_key(
                            &table,
                            &name,
                            index,
                            &mut primary_key,
                            &mut columns,
                        ) {
                            validate_defaults_before(
                                &table,
                                &columns,
                                auto_increment,
                                &default_orders,
                                order,
                            )?;
                            return Err(error);
                        }
                    }
                    TableConstraint::Unique(name) => {
                        let index =
                            match local_constraint_column(&column_indices, &table, &name, "UNIQUE")
                            {
                                Ok(index) => index,
                                Err(error) => {
                                    validate_defaults_before(
                                        &table,
                                        &columns,
                                        auto_increment,
                                        &default_orders,
                                        order,
                                    )?;
                                    return Err(error);
                                }
                            };
                        if let Err(error) =
                            declare_unique(&name, index, &mut saw_unique, &mut unique_columns)
                        {
                            validate_defaults_before(
                                &table,
                                &columns,
                                auto_increment,
                                &default_orders,
                                order,
                            )?;
                            return Err(error);
                        }
                    }
                    TableConstraint::ForeignKey { column, reference } => {
                        let index = match local_constraint_column(
                            &column_indices,
                            &table,
                            &column,
                            "FOREIGN KEY",
                        ) {
                            Ok(index) => index,
                            Err(error) => {
                                validate_defaults_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    &default_orders,
                                    order,
                                )?;
                                return Err(error);
                            }
                        };
                        if let Err(error) = declare_foreign_key(
                            &column,
                            "FOREIGN KEY",
                            index,
                            reference.table,
                            reference.column,
                            &mut saw_foreign_key,
                            &mut foreign_keys,
                        ) {
                            validate_defaults_before(
                                &table,
                                &columns,
                                auto_increment,
                                &default_orders,
                                order,
                            )?;
                            return Err(error);
                        }
                        foreign_key_orders.push(order);
                    }
                }
            }
        }
    }

    unique_columns.retain(|column| Some(*column) != primary_key);
    unique_columns.sort_unstable();

    let mut schema = TableSchema {
        name: table,
        columns,
        primary_key,
        unique_columns,
        foreign_keys: Vec::new(),
    };
    let mut next_default = 0;
    for (foreign_key, order) in foreign_keys.iter().zip(foreign_key_orders) {
        if let Err(error) = validate_foreign_key(catalog, &schema, foreign_key) {
            let earlier_auto_increment = auto_increment
                .filter(|_| auto_increment_order.is_some_and(|auto_order| auto_order < order));
            validate_defaults_from(
                &schema.name,
                &schema.columns,
                earlier_auto_increment,
                &default_orders,
                order,
                next_default,
            )?;
            return Err(error);
        }
    }
    foreign_keys.sort_by_key(|foreign_key| foreign_key.column);
    schema.foreign_keys = foreign_keys;
    if let Some(column) = auto_increment {
        let order = auto_increment_order.expect("auto-increment declarations retain their order");
        next_default = validate_defaults_from(
            &schema.name,
            &schema.columns,
            auto_increment,
            &default_orders,
            order,
            next_default,
        )?;
        validate_auto_increment(&schema, column)?;
    }
    validate_defaults_from(
        &schema.name,
        &schema.columns,
        auto_increment,
        &default_orders,
        usize::MAX,
        next_default,
    )?;
    Ok(ResolvedCreate {
        schema,
        auto_increment,
    })
}

fn validate_defaults_before(
    table: &str,
    columns: &[SchemaColumn],
    auto_increment: Option<usize>,
    default_orders: &[Option<usize>],
    before: usize,
) -> Result<()> {
    validate_defaults_from(table, columns, auto_increment, default_orders, before, 0).map(|_| ())
}

fn validate_defaults_from(
    table: &str,
    columns: &[SchemaColumn],
    auto_increment: Option<usize>,
    default_orders: &[Option<usize>],
    before: usize,
    mut next: usize,
) -> Result<usize> {
    if let Some(index) = auto_increment
        && index < next
        && default_orders[index].is_some_and(|order| order < before)
    {
        validate_default(table, &columns[index], true)?;
    }

    while next < columns.len() {
        let index = next;
        let Some(order) = default_orders[index] else {
            next += 1;
            continue;
        };
        if order >= before {
            break;
        }
        validate_default(table, &columns[index], auto_increment == Some(index))?;
        next += 1;
    }
    Ok(next)
}

fn validate_default(table: &str, column: &SchemaColumn, auto_increment: bool) -> Result<()> {
    #[cfg(test)]
    record_default_validation();
    let default = column
        .default
        .as_ref()
        .expect("DEFAULT declaration orders have matching values");
    if matches!(default, Value::Null) && !column.nullable {
        return Err(Error::Schema(format!(
            "DEFAULT NULL is invalid for NOT NULL column {table:?}.{:?}",
            column.name
        )));
    }
    if !matches!(default, Value::Null) {
        validate_value(default, column)?;
    }
    if auto_increment {
        return Err(Error::Schema(format!(
            "auto-increment column {table:?}.{:?} cannot have a DEFAULT",
            column.name
        )));
    }
    Ok(())
}

#[cfg(test)]
std::thread_local! {
    static DEFAULT_VALIDATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_default_validation() {
    DEFAULT_VALIDATIONS.with(|validations| validations.set(validations.get() + 1));
}

#[cfg(test)]
pub(super) fn reset_default_validations() {
    DEFAULT_VALIDATIONS.with(|validations| validations.set(0));
}

#[cfg(test)]
pub(super) fn default_validations() -> usize {
    DEFAULT_VALIDATIONS.with(std::cell::Cell::get)
}

fn local_constraint_column(
    column_indices: &BTreeMap<String, usize>,
    table: &str,
    column: &str,
    constraint: &str,
) -> Result<usize> {
    column_indices.get(column).copied().ok_or_else(|| {
        Error::Schema(format!(
            "{constraint} references unknown column {column:?} in table {table:?}"
        ))
    })
}
