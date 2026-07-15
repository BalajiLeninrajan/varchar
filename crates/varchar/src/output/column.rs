use crate::value::DataType;

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

    /// Whether the source column was declared nullable.
    ///
    /// This describes schema metadata, not the values in this particular
    /// result. Predicates and inner joins may eliminate all `NULL` values.
    #[must_use]
    pub const fn nullable(&self) -> bool {
        self.nullable
    }
}
