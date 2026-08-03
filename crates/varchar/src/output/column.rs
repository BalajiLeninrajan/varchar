use crate::value::DataType;

/// Engine-produced provenance for a result column.
///
/// Physical projections name their source table and column. Statement-defined
/// virtual columns use a stable synthetic origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnOrigin {
    table: String,
    column: String,
}

impl ColumnOrigin {
    pub(crate) fn new(table: String, column: String) -> Self {
        Self { table, column }
    }

    /// The physical or synthetic origin table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// The physical or synthetic origin column name.
    #[must_use]
    pub fn column(&self) -> &str {
        &self.column
    }
}

/// Engine-produced metadata for one result column.
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

    /// The physical or synthetic origin of this result column.
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
    ///
    /// For a physical projection, this follows the source schema even when
    /// predicates or inner joins eliminate `NULL` from a particular result.
    /// Statement-defined virtual columns declare their result nullability.
    #[must_use]
    pub const fn nullable(&self) -> bool {
        self.nullable
    }
}
