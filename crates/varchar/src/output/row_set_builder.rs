use super::{ColumnOrigin, ResultColumn, RowSet};
use crate::expression::{format_value, format_value_len};
use crate::limits::ByteBudget;
use crate::{DataType, Error, Resource, Result, Value};

pub(crate) struct ResultColumnSpec<'a> {
    pub(crate) label: &'a str,
    pub(crate) origin_table: &'a str,
    pub(crate) origin_column: &'a str,
    pub(crate) data_type: DataType,
    pub(crate) nullable: bool,
}

pub(crate) enum ResultCell<'a> {
    Text(&'a str),
    Boolean(bool),
    Null,
    /// Renders a value as the SQL literal that parses back to it, so a TEXT
    /// value spelled `NULL` stays distinguishable from an actual `NULL`.
    DisplayValue(&'a Value),
}

pub(crate) struct RowSetBuilder {
    columns: Vec<ResultColumn>,
    rows: Vec<Vec<Value>>,
    row_structure_bytes: usize,
    output_budget: ByteBudget,
}

impl RowSetBuilder {
    pub(crate) fn new(specs: &[ResultColumnSpec<'_>], limit: usize) -> Result<Self> {
        let mut output_budget = ByteBudget::new(limit, Resource::QueryOutputBytes);
        output_budget.charge(std::mem::size_of::<RowSet>())?;

        let column_bytes = specs
            .len()
            .checked_mul(std::mem::size_of::<ResultColumn>())
            .ok_or_else(|| output_budget.limit_error())?;
        output_budget.charge(column_bytes)?;
        let mut columns = Vec::new();
        columns
            .try_reserve_exact(specs.len())
            .map_err(|_| allocation_error("reserving result columns"))?;
        for spec in specs {
            let label = clone_output_text(spec.label, &mut output_budget)?;
            let origin_table = clone_output_text(spec.origin_table, &mut output_budget)?;
            let origin_column = clone_output_text(spec.origin_column, &mut output_budget)?;
            columns.push(ResultColumn::new(
                label,
                ColumnOrigin::new(origin_table, origin_column),
                spec.data_type,
                spec.nullable,
            ));
        }

        let value_slots = specs
            .len()
            .checked_mul(std::mem::size_of::<Value>())
            .ok_or_else(|| output_budget.limit_error())?;
        let row_descriptors = std::mem::size_of::<Vec<Value>>()
            .checked_mul(4)
            .ok_or_else(|| output_budget.limit_error())?;
        let row_structure_bytes = row_descriptors
            .checked_add(value_slots)
            .ok_or_else(|| output_budget.limit_error())?;

        Ok(Self {
            columns,
            rows: Vec::new(),
            row_structure_bytes,
            output_budget,
        })
    }

    pub(crate) fn push(&mut self, cells: &[ResultCell<'_>]) -> Result<()> {
        if cells.len() != self.columns.len() {
            return Err(Error::Capacity {
                operation: "materializing a result row with the declared width",
            });
        }
        let payload_bytes = cells.iter().try_fold(0_usize, |total, cell| {
            total
                .checked_add(cell.payload_bytes())
                .ok_or_else(|| self.output_budget.limit_error())
        })?;
        let row_bytes = self
            .row_structure_bytes
            .checked_add(payload_bytes)
            .ok_or_else(|| self.output_budget.limit_error())?;
        self.output_budget.charge(row_bytes)?;

        self.rows
            .try_reserve(1)
            .map_err(|_| allocation_error("reserving result rows"))?;
        let mut row = Vec::new();
        row.try_reserve_exact(cells.len())
            .map_err(|_| allocation_error("reserving result values"))?;
        for cell in cells {
            row.push(cell.materialize()?);
        }
        self.rows.push(row);
        Ok(())
    }

    pub(crate) fn finish(self) -> RowSet {
        RowSet::new(self.columns, self.rows)
    }
}

impl ResultCell<'_> {
    fn payload_bytes(&self) -> usize {
        match self {
            Self::Text(value) => value.len(),
            Self::Boolean(_) | Self::Null => 0,
            Self::DisplayValue(value) => format_value_len(value),
        }
    }

    fn materialize(&self) -> Result<Value> {
        match self {
            Self::Text(value) => clone_text_value(value),
            Self::Boolean(value) => Ok(Value::Boolean(*value)),
            Self::Null => Ok(Value::Null),
            Self::DisplayValue(value) => display_value(value).map(Value::Text),
        }
    }
}

fn clone_output_text(value: &str, budget: &mut ByteBudget) -> Result<String> {
    budget.charge(value.len())?;
    clone_string(value, "cloning result column metadata")
}

fn clone_text_value(value: &str) -> Result<Value> {
    clone_string(value, "cloning result text").map(Value::Text)
}

fn clone_string(value: &str, operation: &'static str) -> Result<String> {
    let mut cloned = String::new();
    cloned
        .try_reserve_exact(value.len())
        .map_err(|_| allocation_error(operation))?;
    cloned.push_str(value);
    Ok(cloned)
}

/// Renders a value as the SQL literal a client can feed straight back in.
///
/// The literal spelling is what keeps `DEFAULT NULL`, `DEFAULT 'NULL'` and a
/// column with no default apart in metadata results: the first two arrive here
/// as `Value::Null` and `Value::Text("NULL")` and render as `NULL` and
/// `'NULL'`, while the third never reaches this cell at all.
fn display_value(value: &Value) -> Result<String> {
    let length = format_value_len(value);
    let mut output = String::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| allocation_error("formatting result text"))?;
    format_value(&mut output, value).map_err(|_| Error::Capacity {
        operation: "formatting a result value as a SQL literal",
    })?;
    debug_assert_eq!(output.len(), length);
    Ok(output)
}

const fn allocation_error(operation: &'static str) -> Error {
    Error::Allocation { operation }
}
