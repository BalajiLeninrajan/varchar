use crate::output::{ResultCell, ResultColumnSpec, RowSetBuilder};
use crate::resolve::require_table;
use crate::sql::DescribeTable;
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
        result.push(&[ResultCell::Text(table)])?;
    }
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
        result.push(&[
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
