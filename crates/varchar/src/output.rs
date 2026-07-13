//! Engine-produced result snapshots returned by the public database facade.

use crate::value::{DataType, Value};

/// Engine-produced provenance for a projected result column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnOrigin {
    table: String,
    column: String,
}

impl ColumnOrigin {
    pub(crate) fn new(table: String, column: String) -> Self {
        Self { table, column }
    }

    /// The source table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// The source column name.
    #[must_use]
    pub fn column(&self) -> &str {
        &self.column
    }
}

/// Engine-produced metadata for one projected result column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultColumn {
    label: String,
    origin: ColumnOrigin,
    data_type: DataType,
    nullable: bool,
}

impl ResultColumn {
    pub(crate) fn new(
        label: String,
        origin: ColumnOrigin,
        data_type: DataType,
        nullable: bool,
    ) -> Self {
        Self {
            label,
            origin,
            data_type,
            nullable,
        }
    }

    /// The display label for this result column.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The table column that supplied this result column.
    #[must_use]
    pub fn origin(&self) -> &ColumnOrigin {
        &self.origin
    }

    /// The SQL data type of this result column.
    #[must_use]
    pub const fn data_type(&self) -> DataType {
        self.data_type
    }

    /// Whether this result column can contain `NULL`.
    #[must_use]
    pub const fn nullable(&self) -> bool {
        self.nullable
    }
}

/// An immutable, materialized snapshot returned by a `SELECT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowSet {
    columns: Vec<ResultColumn>,
    rows: Vec<Vec<Value>>,
}

impl RowSet {
    pub(crate) fn new(columns: Vec<ResultColumn>, rows: Vec<Vec<Value>>) -> Self {
        debug_assert!(
            rows.iter().all(|row| row.len() == columns.len()),
            "every result row must match the projected column width"
        );
        Self { columns, rows }
    }

    /// Projected columns, in query order and including duplicates.
    #[must_use]
    pub fn columns(&self) -> &[ResultColumn] {
        &self.columns
    }

    /// Materialized rows in deterministic query order.
    #[must_use]
    pub fn rows(&self) -> &[Vec<Value>] {
        &self.rows
    }

    /// Consume this result and return its materialized rows.
    #[must_use]
    pub fn into_rows(self) -> Vec<Vec<Value>> {
        self.rows
    }

    /// Consume this result and return its column metadata and rows.
    #[must_use]
    pub fn into_parts(self) -> (Vec<ResultColumn>, Vec<Vec<Value>>) {
        (self.columns, self.rows)
    }
}

/// An immutable explanation of the source-row scan produced for a `SELECT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectExplanation {
    pattern: String,
    sources: Vec<String>,
    columns: Vec<ResultColumn>,
}

impl SelectExplanation {
    pub(crate) fn new(pattern: String, sources: Vec<String>, columns: Vec<ResultColumn>) -> Self {
        Self {
            pattern,
            sources,
            columns,
        }
    }

    /// The generated pattern used to select complete encoded rows.
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Source tables in `FROM`/`JOIN` order.
    #[must_use]
    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// Projected columns, in query order and including duplicates.
    #[must_use]
    pub fn columns(&self) -> &[ResultColumn] {
        &self.columns
    }
}

/// The result of executing one statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Rows(RowSet),
    Affected { rows: usize },
    Created { table: String },
    Explain(SelectExplanation),
}

impl Outcome {
    /// Whether this result came from a statement that mutates the database.
    #[must_use]
    pub const fn is_mutation(&self) -> bool {
        matches!(self, Self::Affected { .. } | Self::Created { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::{ColumnOrigin, ResultColumn, RowSet, SelectExplanation};
    use crate::value::{DataType, Value};

    #[test]
    fn snapshots_expose_read_and_consuming_views() {
        let origin = ColumnOrigin::new(String::from("items"), String::from("id"));
        let column = ResultColumn::new(String::from("id"), origin, DataType::Integer, false);
        let row_set = RowSet::new(vec![column.clone()], vec![vec![Value::Integer(1)]]);
        let explanation = SelectExplanation::new(
            String::from("row-pattern"),
            vec![String::from("items")],
            vec![column],
        );

        assert_eq!(row_set.columns()[0].label(), "id");
        assert_eq!(row_set.columns()[0].origin().table(), "items");
        assert_eq!(row_set.columns()[0].origin().column(), "id");
        assert_eq!(row_set.columns()[0].data_type(), DataType::Integer);
        assert!(!row_set.columns()[0].nullable());
        assert_eq!(row_set.rows(), &[vec![Value::Integer(1)]]);
        assert_eq!(row_set.clone().into_rows(), vec![vec![Value::Integer(1)]]);
        let (columns, rows) = row_set.into_parts();
        assert_eq!(columns.len(), 1);
        assert_eq!(rows, vec![vec![Value::Integer(1)]]);

        assert_eq!(explanation.pattern(), "row-pattern");
        assert_eq!(explanation.sources(), &["items"]);
        assert_eq!(explanation.columns()[0].label(), "id");
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic]
    fn row_set_rejects_inconsistent_row_widths_in_debug_builds() {
        RowSet::new(
            vec![ResultColumn::new(
                String::from("id"),
                ColumnOrigin::new(String::from("items"), String::from("id")),
                DataType::Integer,
                false,
            )],
            vec![vec![Value::Integer(1), Value::Integer(2)]],
        );
    }
}
