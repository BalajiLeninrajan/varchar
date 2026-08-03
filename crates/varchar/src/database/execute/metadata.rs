mod show_create;

use crate::output::{ResultCell, ResultColumnSpec, RowSetBuilder};
use crate::resolve::require_table;
use crate::sql::{DescribeTable, ShowCreateTable};
use crate::storage::Catalog;
use crate::{DataType, Result, RowSet};

const TABLES_ORIGIN: &str = "information_schema.tables";
const COLUMNS_ORIGIN: &str = "information_schema.columns";

pub(super) fn show_tables(catalog: &Catalog, output_limit: usize) -> Result<RowSet> {
    let columns = [ResultColumnSpec {
        label: "table_name",
        origin_table: TABLES_ORIGIN,
        origin_column: "table_name",
        data_type: DataType::Text,
        nullable: false,
    }];
    let mut result = RowSetBuilder::new(&columns, output_limit)?;
    for (table, _) in catalog.tables() {
        result.push([ResultCell::Text(table)])?;
    }
    Ok(result.finish())
}

pub(super) fn show_create_table(
    catalog: &Catalog,
    statement: &ShowCreateTable,
    output_limit: usize,
) -> Result<RowSet> {
    let table = require_table(catalog, &statement.table)?;
    let columns = [
        ResultColumnSpec {
            label: "table_name",
            origin_table: TABLES_ORIGIN,
            origin_column: "table_name",
            data_type: DataType::Text,
            nullable: false,
        },
        ResultColumnSpec {
            label: "create_statement",
            origin_table: TABLES_ORIGIN,
            origin_column: "create_statement",
            data_type: DataType::Text,
            nullable: false,
        },
    ];
    let mut result = RowSetBuilder::new(&columns, output_limit)?;
    let auto_increment = catalog.auto_increment(&statement.table);
    let statement_length =
        show_create::measure(table, auto_increment)?.ok_or_else(|| result.limit_error())?;
    let payload_bytes = table
        .name
        .len()
        .checked_add(statement_length)
        .ok_or_else(|| result.limit_error())?;
    result.preflight_row_payload(payload_bytes)?;
    let create_statement = show_create::render(table, auto_increment, statement_length)?;
    result.push([
        ResultCell::Text(&table.name),
        ResultCell::OwnedText(create_statement),
    ])?;
    Ok(result.finish())
}

pub(super) fn describe_table(
    catalog: &Catalog,
    statement: &DescribeTable,
    output_limit: usize,
) -> Result<RowSet> {
    let table = require_table(catalog, &statement.table)?;
    let columns = [
        column_spec("column_name", DataType::Text, false),
        column_spec("data_type", DataType::Text, false),
        column_spec("nullable", DataType::Boolean, false),
        column_spec("primary_key", DataType::Boolean, false),
        column_spec("unique", DataType::Boolean, false),
        column_spec("default_value", DataType::Text, true),
        column_spec("auto_increment", DataType::Boolean, false),
    ];
    let mut result = RowSetBuilder::new(&columns, output_limit)?;
    let auto_increment = catalog.auto_increment(&statement.table);

    for (index, column) in table.columns.iter().enumerate() {
        let primary_key = table.primary_key == Some(index);
        let unique = primary_key || table.unique_columns.binary_search(&index).is_ok();
        let default = column
            .default
            .as_ref()
            .map_or(ResultCell::Null, ResultCell::DisplayValue);
        result.push([
            ResultCell::Text(&column.name),
            ResultCell::Text(data_type_name(column.data_type)),
            ResultCell::Boolean(column.nullable),
            ResultCell::Boolean(primary_key),
            ResultCell::Boolean(unique),
            default,
            ResultCell::Boolean(auto_increment.is_some_and(|state| state.column == index)),
        ])?;
    }

    Ok(result.finish())
}

const fn column_spec(
    name: &'static str,
    data_type: DataType,
    nullable: bool,
) -> ResultColumnSpec<'static> {
    ResultColumnSpec {
        label: name,
        origin_table: COLUMNS_ORIGIN,
        origin_column: name,
        data_type,
        nullable,
    }
}

const fn data_type_name(data_type: DataType) -> &'static str {
    match data_type {
        DataType::Text => "TEXT",
        DataType::Integer => "INTEGER",
        DataType::Boolean => "BOOLEAN",
    }
}
