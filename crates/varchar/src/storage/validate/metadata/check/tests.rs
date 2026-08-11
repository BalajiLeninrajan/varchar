use super::super::reserve_check_program;
use super::{Field, decode_program, decode_usize};
use crate::expression::{CheckPredicate, CheckProgram, CheckProgramNode};
use crate::storage::TableSchema;
use crate::storage::budget::WorkingBudget;
use crate::storage::decode::CheckMetadata;
use crate::{DataType, Error, Resource, SchemaColumn};

fn single_column_schema(data_type: DataType) -> TableSchema {
    TableSchema {
        name: String::from("t"),
        columns: vec![SchemaColumn {
            name: String::from("value"),
            data_type,
            nullable: true,
            default: None,
        }],
        primary_key: None,
        unique_columns: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
    }
}

fn text_schema() -> TableSchema {
    single_column_schema(DataType::Text)
}

fn integer_schema() -> TableSchema {
    single_column_schema(DataType::Integer)
}

fn leaf_check_program() -> CheckProgram {
    CheckProgram::new(vec![CheckProgramNode::Predicate(CheckPredicate::IsNull {
        column: 0,
    })])
}

#[test]
fn retained_check_programs_charge_logical_descriptors_not_vector_capacity() {
    let descriptor_bytes = std::mem::size_of::<CheckProgram>();
    let exact = 3 * descriptor_bytes;
    let mut checks = Vec::new();
    let mut exact_budget = WorkingBudget::new(exact);
    for _ in 0..3 {
        reserve_check_program(&mut checks, &mut exact_budget)
            .expect("three logical CHECK descriptors fit exactly");
        checks.push(leaf_check_program());
    }
    assert_eq!(checks.len(), 3);
    assert!(matches!(
        reserve_check_program(&mut checks, &mut exact_budget),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit,
        }) if limit == exact
    ));
    assert_eq!(checks.len(), 3, "a failed reservation inserts nothing");

    let one_under_limit = exact - 1;
    let mut checks = Vec::new();
    let mut one_under = WorkingBudget::new(one_under_limit);
    for _ in 0..2 {
        reserve_check_program(&mut checks, &mut one_under)
            .expect("two descriptors fit below the three-item boundary");
        checks.push(leaf_check_program());
    }
    assert!(matches!(
        reserve_check_program(&mut checks, &mut one_under),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit,
        }) if limit == one_under_limit
    ));
    assert_eq!(
        checks.len(),
        2,
        "the rejected third descriptor is not inserted"
    );
}

#[test]
fn persisted_check_numeric_fields_use_a_u32_grammar() {
    const OFFSET: usize = 26;
    const MAX_U32: &str = "4294967295";
    const ABOVE_U32: &str = "4294967296";

    for message in [
        "invalid CHECK child count",
        "invalid CHECK LIKE atom count",
        "invalid CHECK IN item count",
        "invalid CHECK column position",
    ] {
        assert_eq!(
            decode_usize(
                Field {
                    text: MAX_U32,
                    offset: OFFSET,
                },
                message,
            )
            .expect("u32::MAX is in the persisted numeric grammar"),
            usize::try_from(u32::MAX).expect("supported targets represent u32 positions"),
        );
        assert!(matches!(
            decode_usize(
                Field {
                    text: ABOVE_U32,
                    offset: OFFSET,
                },
                message,
            ),
            Err(Error::CorruptStorage { offset, message: actual })
                if offset == OFFSET && actual == message
        ));
    }

    let schema = integer_schema();
    for (column, expected) in [
        (MAX_U32, "CHECK column position is outside its table"),
        (ABOVE_U32, "invalid CHECK column position"),
    ] {
        let program = format!("EQ|{column}|I1");
        assert!(matches!(
            decode_program(
                &schema,
                CheckMetadata {
                    table: "t",
                    program: &program,
                    program_offset: OFFSET - "EQ|".len(),
                },
                0,
                usize::MAX,
                &mut WorkingBudget::new(0),
            ),
            Err(Error::CorruptStorage { offset, message })
                if offset == OFFSET && message == expected
        ));
    }
}

#[test]
fn predicate_limit_is_checked_before_reconstruction_budget() {
    let schema = text_schema();
    for (program, existing, max_predicates) in [
        ("IN|0|2|Tone|Ttwo", 1, 2),
        ("AND|2|EQ|0|Tone|EQ|0|Ttwo", 0, 1),
    ] {
        let error = decode_program(
            &schema,
            CheckMetadata {
                table: "t",
                program,
                program_offset: 0,
            },
            existing,
            max_predicates,
            &mut WorkingBudget::new(0),
        )
        .expect_err("the cumulative predicate limit is checked before reconstruction");

        assert!(matches!(
            error,
            Error::ResourceLimit {
                resource: Resource::CheckPredicates,
                limit,
            } if limit == max_predicates
        ));
    }
}

#[test]
fn malformed_programs_are_rejected_before_working_budget_is_consumed() {
    let schema = text_schema();
    let program = "AND|2|IN|0|3|Tone|Ttwo|Tthree|BAD";
    let error = decode_program(
        &schema,
        CheckMetadata {
            table: "t",
            program,
            program_offset: 11,
        },
        0,
        0,
        &mut WorkingBudget::new(0),
    )
    .expect_err("late malformed storage must not be masked by the working limit");

    assert!(matches!(
        error,
        Error::CorruptStorage { offset, message }
            if offset == 11 + program.find("BAD").expect("BAD opcode exists")
                && message == "unknown CHECK program opcode"
    ));
}
