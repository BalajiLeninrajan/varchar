use crate::output::{ResultCell, ResultColumnSpec, RowSetBuilder};
use crate::storage::Catalog;
use crate::{DataType, Result, RowSet};

const TABLES_ORIGIN: &str = "information_schema.tables";

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
