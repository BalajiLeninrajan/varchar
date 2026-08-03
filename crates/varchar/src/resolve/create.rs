//! `CREATE TABLE` resolution and validated schema assembly.

mod auto_increment;
mod check;
mod foreign_key;
mod primary_key;
mod unique;

use std::collections::BTreeMap;

use auto_increment::{declare_auto_increment, validate_auto_increment};
use check::resolve_check;
use foreign_key::{declare_foreign_key, validate_foreign_key, validate_set_null_column};
use primary_key::declare_primary_key;
use unique::declare_unique;

use crate::expression::CheckProgram;
use crate::limits::check_limit;
use crate::sql::{
    ColumnModifier, CreateElement, CreateTable, Expression,
    ForeignKeyDeleteAction as ParsedDeleteAction, ForeignKeyUpdateAction as ParsedUpdateAction,
    TableConstraint,
};
use crate::storage::{
    Catalog, ForeignKey, ForeignKeyDeleteAction, ForeignKeyUpdateAction, TableSchema,
};
use crate::value::validate_value;
use crate::{Error, Resource, Result, SchemaColumn, Value};

pub(crate) struct ResolvedCreate {
    pub(crate) schema: TableSchema,
    pub(crate) auto_increment: Option<usize>,
}

enum SemanticEvent {
    Default {
        order: usize,
        column: usize,
    },
    Check {
        order: usize,
        expression: Expression,
    },
    SetNull {
        order: usize,
        column: usize,
    },
}

impl SemanticEvent {
    const fn order(&self) -> usize {
        match self {
            Self::Default { order, .. }
            | Self::Check { order, .. }
            | Self::SetNull { order, .. } => *order,
        }
    }
}

struct SemanticResolution {
    events: Vec<SemanticEvent>,
    next_event: usize,
    check_predicates: usize,
    checks: Vec<CheckProgram>,
    max_predicates: usize,
}

impl SemanticResolution {
    fn new(column_count: usize, check_count: usize, max_predicates: usize) -> Result<Self> {
        let event_capacity = column_count
            .checked_mul(2)
            .and_then(|count| count.checked_add(check_count))
            .ok_or(Error::Capacity {
                operation: "counting CREATE semantic events",
            })?;
        let mut events = Vec::new();
        events
            .try_reserve_exact(event_capacity)
            .map_err(|_| Error::Allocation {
                operation: "reserving CREATE semantic events",
            })?;
        Ok(Self {
            events,
            next_event: 0,
            check_predicates: 0,
            checks: Vec::new(),
            max_predicates,
        })
    }

    fn queue_default(&mut self, order: usize, column: usize) {
        self.events.push(SemanticEvent::Default { order, column });
    }

    fn queue_check(&mut self, order: usize, expression: Expression) {
        self.events.push(SemanticEvent::Check { order, expression });
    }

    fn queue_set_null(&mut self, order: usize, column: usize) {
        self.events.push(SemanticEvent::SetNull { order, column });
    }

    fn drain_before(
        &mut self,
        table: &str,
        columns: &[SchemaColumn],
        auto_increment: Option<usize>,
        before: usize,
    ) -> Result<()> {
        self.drain_while(table, columns, auto_increment, |order| order < before)
    }

    fn drain_while(
        &mut self,
        table: &str,
        columns: &[SchemaColumn],
        auto_increment: Option<usize>,
        should_drain: impl Fn(usize) -> bool,
    ) -> Result<()> {
        while let Some(event) = self.events.get(self.next_event) {
            if !should_drain(event.order()) {
                break;
            }
            match event {
                SemanticEvent::Default { column, .. } => {
                    validate_default(table, &columns[*column], auto_increment == Some(*column))?;
                }
                SemanticEvent::Check { expression, .. } => {
                    let predicates = expression.predicate_units()?;
                    let total =
                        self.check_predicates
                            .checked_add(predicates)
                            .ok_or(Error::Capacity {
                                operation: "counting table CHECK predicates",
                            })?;
                    check_limit(total, self.max_predicates, Resource::CheckPredicates)?;
                    self.checks.try_reserve(1).map_err(|_| Error::Allocation {
                        operation: "reserving resolved CHECK declarations",
                    })?;
                    self.checks.push(resolve_check(table, columns, expression)?);
                    self.check_predicates = total;
                }
                SemanticEvent::SetNull { column, .. } => {
                    validate_set_null_column(table, &columns[*column])?;
                }
            }
            self.next_event += 1;
        }
        Ok(())
    }

    fn into_checks(self) -> Vec<CheckProgram> {
        self.checks
    }
}

#[cfg(test)]
pub(crate) fn create_schema(catalog: &Catalog, statement: CreateTable) -> Result<ResolvedCreate> {
    create_schema_with_limit(catalog, statement, usize::MAX)
}

pub(crate) fn create_schema_with_limit(
    catalog: &Catalog,
    statement: CreateTable,
    max_predicates: usize,
) -> Result<ResolvedCreate> {
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
    let check_count = elements.iter().try_fold(0_usize, |count, element| {
        let declarations = match element {
            CreateElement::Column(column) => column
                .modifiers
                .iter()
                .filter(|modifier| matches!(modifier, ColumnModifier::Check(_)))
                .count(),
            CreateElement::Constraint(TableConstraint::Check(_)) => 1,
            CreateElement::Constraint(_) => 0,
        };
        count.checked_add(declarations).ok_or(Error::Capacity {
            operation: "counting CHECK declarations",
        })
    })?;
    let mut semantic_resolution =
        SemanticResolution::new(columns.len(), check_count, max_predicates)?;
    let mut auto_increment = None;
    let mut auto_increment_order = None;
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
                                semantic_resolution.drain_before(
                                    &table,
                                    &columns,
                                    auto_increment,
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
                                semantic_resolution.drain_before(
                                    &table,
                                    &columns,
                                    auto_increment,
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
                                semantic_resolution.drain_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    order,
                                )?;
                                return Err(error);
                            }
                        }
                        ColumnModifier::References(reference) => {
                            let on_delete = resolve_delete_action(reference.on_delete);
                            if let Err(error) = declare_foreign_key(
                                &column.name,
                                "REFERENCES",
                                ForeignKey {
                                    column: index,
                                    referenced_table: reference.table,
                                    referenced_column: reference.column,
                                    on_delete,
                                    on_update: resolve_update_action(reference.on_update),
                                },
                                &mut saw_foreign_key,
                                &mut foreign_keys,
                            ) {
                                semantic_resolution.drain_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    order,
                                )?;
                                return Err(error);
                            }
                            if on_delete == ForeignKeyDeleteAction::SetNull {
                                semantic_resolution.queue_set_null(order, index);
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
                                semantic_resolution.drain_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    order,
                                )?;
                                return Err(error);
                            }
                            auto_increment_order = Some(order);
                        }
                        ColumnModifier::Default(value) => {
                            if columns[index].default.is_some() {
                                semantic_resolution.drain_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    order,
                                )?;
                                return Err(Error::Schema(format!(
                                    "duplicate DEFAULT declaration for column {:?}",
                                    column.name
                                )));
                            }
                            columns[index].default = Some(value);
                            semantic_resolution.queue_default(order, index);
                        }
                        ColumnModifier::Check(expression) => {
                            semantic_resolution.queue_check(order, expression);
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
                                semantic_resolution.drain_before(
                                    &table,
                                    &columns,
                                    auto_increment,
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
                            semantic_resolution.drain_before(
                                &table,
                                &columns,
                                auto_increment,
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
                                    semantic_resolution.drain_before(
                                        &table,
                                        &columns,
                                        auto_increment,
                                        order,
                                    )?;
                                    return Err(error);
                                }
                            };
                        if let Err(error) =
                            declare_unique(&name, index, &mut saw_unique, &mut unique_columns)
                        {
                            semantic_resolution.drain_before(
                                &table,
                                &columns,
                                auto_increment,
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
                                semantic_resolution.drain_before(
                                    &table,
                                    &columns,
                                    auto_increment,
                                    order,
                                )?;
                                return Err(error);
                            }
                        };
                        let on_delete = resolve_delete_action(reference.on_delete);
                        if let Err(error) = declare_foreign_key(
                            &column,
                            "FOREIGN KEY",
                            ForeignKey {
                                column: index,
                                referenced_table: reference.table,
                                referenced_column: reference.column,
                                on_delete,
                                on_update: resolve_update_action(reference.on_update),
                            },
                            &mut saw_foreign_key,
                            &mut foreign_keys,
                        ) {
                            semantic_resolution.drain_before(
                                &table,
                                &columns,
                                auto_increment,
                                order,
                            )?;
                            return Err(error);
                        }
                        if on_delete == ForeignKeyDeleteAction::SetNull {
                            semantic_resolution.queue_set_null(order, index);
                        }
                        foreign_key_orders.push(order);
                    }
                    TableConstraint::Check(expression) => {
                        semantic_resolution.queue_check(order, expression);
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
        checks: Vec::new(),
    };
    for (foreign_key, order) in foreign_keys.iter().zip(foreign_key_orders) {
        if let Err(error) = validate_foreign_key(catalog, &schema, foreign_key) {
            let earlier_auto_increment = auto_increment
                .filter(|_| auto_increment_order.is_some_and(|auto_order| auto_order < order));
            semantic_resolution.drain_before(
                &schema.name,
                &schema.columns,
                earlier_auto_increment,
                order,
            )?;
            return Err(error);
        }
    }
    foreign_keys.sort_by_key(|foreign_key| foreign_key.column);
    schema.foreign_keys = foreign_keys;
    if let Some(column) = auto_increment {
        let order = auto_increment_order.expect("auto-increment declarations retain their order");
        semantic_resolution.drain_before(&schema.name, &schema.columns, auto_increment, order)?;
        validate_auto_increment(&schema, column)?;
    }
    semantic_resolution.drain_before(&schema.name, &schema.columns, auto_increment, usize::MAX)?;
    schema.checks = semantic_resolution.into_checks();
    Ok(ResolvedCreate {
        schema,
        auto_increment,
    })
}

const fn resolve_delete_action(action: ParsedDeleteAction) -> ForeignKeyDeleteAction {
    match action {
        ParsedDeleteAction::Restrict => ForeignKeyDeleteAction::Restrict,
        ParsedDeleteAction::Cascade => ForeignKeyDeleteAction::Cascade,
        ParsedDeleteAction::SetNull => ForeignKeyDeleteAction::SetNull,
    }
}

const fn resolve_update_action(action: ParsedUpdateAction) -> ForeignKeyUpdateAction {
    match action {
        ParsedUpdateAction::Restrict => ForeignKeyUpdateAction::Restrict,
        ParsedUpdateAction::Cascade => ForeignKeyUpdateAction::Cascade,
    }
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
