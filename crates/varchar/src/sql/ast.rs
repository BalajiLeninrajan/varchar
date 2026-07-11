//! Parsed SQL syntax owned by the parser and consumed by semantic resolution.

use crate::{DataType, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Statement {
    CreateTable(CreateTable),
    Insert(Insert),
    Select(Select),
    Update(Update),
    Delete(Delete),
    ExplainRegex(Select),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreateTable {
    pub(crate) table: String,
    pub(crate) elements: Vec<CreateElement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CreateElement {
    Column(ColumnDef),
    Constraint(TableConstraint),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ColumnDef {
    pub(crate) name: String,
    pub(crate) data_type: DataType,
    pub(crate) modifiers: Vec<ColumnModifier>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ColumnModifier {
    NotNull,
    PrimaryKey,
    References(ForeignKeyReference),
    AutoIncrement,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ForeignKeyReference {
    pub(crate) table: String,
    pub(crate) column: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TableConstraint {
    PrimaryKey(String),
    ForeignKey {
        column: String,
        reference: ForeignKeyReference,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Insert {
    pub(crate) table: String,
    pub(crate) columns: Option<Vec<String>>,
    pub(crate) values: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Select {
    pub(crate) table: String,
    pub(crate) joins: Vec<Join>,
    pub(crate) projection: Projection,
    pub(crate) predicates: Vec<Predicate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Projection {
    All,
    Items(Vec<ProjectionItem>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectionItem {
    Column(ColumnRef),
    QualifiedAll(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ColumnRef {
    pub(crate) qualifier: Option<String>,
    pub(crate) name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Join {
    pub(crate) table: String,
    pub(crate) conditions: Vec<JoinCondition>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JoinCondition {
    pub(crate) left: ColumnRef,
    pub(crate) right: ColumnRef,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Update {
    pub(crate) table: String,
    pub(crate) assignments: Vec<Assignment>,
    pub(crate) predicates: Vec<Predicate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Assignment {
    pub(crate) column: String,
    pub(crate) value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Delete {
    pub(crate) table: String,
    pub(crate) predicates: Vec<Predicate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Predicate {
    pub(crate) column: ColumnRef,
    pub(crate) operator: PredicateOperator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PredicateOperator {
    Equal(Value),
    NotEqual(Value),
    Like(String),
    IsNull,
    IsNotNull,
}
