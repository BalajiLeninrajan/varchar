use super::{ColumnOrigin, ResultColumn, RowSet};
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
        }
    }

    fn materialize(&self) -> Result<Value> {
        match self {
            Self::Text(value) => clone_text_value(value),
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

const fn allocation_error(operation: &'static str) -> Error {
    Error::Allocation { operation }
}
