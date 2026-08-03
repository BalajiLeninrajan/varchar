mod column;
mod create;
mod expression;
mod insert;
mod join;
mod order;
mod predicate;
mod projection;
mod select;
mod source;
mod update;

use crate::sql::{self, Statement};
use crate::storage::{Catalog, StorageState, TableSchema};
use crate::{DataType, SchemaColumn};

fn people_schema() -> TableSchema {
    TableSchema {
        name: String::from("people"),
        columns: vec![
            SchemaColumn {
                name: String::from("id"),
                data_type: DataType::Integer,
                nullable: false,
                default: None,
            },
            SchemaColumn {
                name: String::from("note"),
                data_type: DataType::Text,
                nullable: true,
                default: None,
            },
            SchemaColumn {
                name: String::from("active"),
                data_type: DataType::Boolean,
                nullable: false,
                default: None,
            },
        ],
        primary_key: None,
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
    }
}

fn select_statement(sql: &str) -> crate::sql::Select {
    let Statement::Select(statement) = sql::parse(sql).expect("statement parses") else {
        panic!("expected SELECT");
    };
    statement
}

fn catalog(blob: &str) -> Catalog {
    StorageState::load(blob.to_owned(), usize::MAX)
        .expect("fixture catalog is valid")
        .catalog()
        .clone()
}
