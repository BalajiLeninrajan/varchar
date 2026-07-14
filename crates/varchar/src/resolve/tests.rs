mod column;
mod create;
mod insert;
mod join;
mod predicate;
mod projection;
mod select;
mod source;
mod update;

use crate::sql::{self, Statement};
use crate::storage::{Catalog, StorageState, TableSchema};
use crate::{DataType, ErrorCode, Resource, SchemaColumn};

fn assert_error<T>(result: crate::Result<T>, code: ErrorCode, detail: &str) {
    let Err(error) = result else {
        panic!("expected {code} containing {detail:?}");
    };
    assert_eq!(error.code(), code);
    assert_eq!(error.to_string(), format!("{code}: {detail}"));
}

fn assert_resource_error<T>(result: crate::Result<T>, resource: Resource, limit: usize) {
    let Err(error) = result else {
        panic!("expected a {resource} resource limit");
    };
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
    assert_eq!(error.resource(), Some(resource));
    assert_eq!(error.limit(), Some(limit));
}

fn people_schema() -> TableSchema {
    TableSchema {
        name: String::from("people"),
        columns: vec![
            SchemaColumn {
                name: String::from("id"),
                data_type: DataType::Integer,
                nullable: false,
            },
            SchemaColumn {
                name: String::from("note"),
                data_type: DataType::Text,
                nullable: true,
            },
            SchemaColumn {
                name: String::from("active"),
                data_type: DataType::Boolean,
                nullable: false,
            },
        ],
        primary_key: None,
        foreign_keys: Vec::new(),
    }
}

fn select_statement(sql: &str) -> crate::sql::Select {
    let Statement::Select(statement) = sql::parse(sql).expect("statement parses") else {
        panic!("expected SELECT");
    };
    statement
}

fn catalog(blob: &str) -> Catalog {
    StorageState::load(blob.to_owned())
        .expect("fixture catalog is valid")
        .catalog()
        .clone()
}
