mod create;
mod insert;
mod predicate;
mod update;

use crate::storage::TableSchema;
use crate::{Column, DataType};

fn people_schema() -> TableSchema {
    TableSchema {
        name: String::from("people"),
        columns: vec![
            Column {
                name: String::from("id"),
                data_type: DataType::Integer,
                nullable: false,
            },
            Column {
                name: String::from("note"),
                data_type: DataType::Text,
                nullable: true,
            },
            Column {
                name: String::from("active"),
                data_type: DataType::Boolean,
                nullable: false,
            },
        ],
        primary_key: None,
        foreign_keys: Vec::new(),
    }
}
