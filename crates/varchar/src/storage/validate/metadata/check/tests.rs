use super::super::reserve_check_program;
use super::{Field, ShapeFrame, decode_program, decode_usize};
use crate::expression::{CheckPredicate, CheckProgram, CheckProgramNode, LikeAtom};
use crate::limits::ByteBudget;
use crate::storage::TableSchema;
use crate::storage::decode::CheckMetadata;
use crate::{DataType, Error, Resource, SchemaColumn, Value};

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

fn alternating_binary_program(depth: usize, predicate: &str) -> String {
    let mut program = String::with_capacity(depth * (6 + predicate.len()) + predicate.len());
    for level in 0..depth {
        program.push_str(if level % 2 == 0 { "AND|2|" } else { "OR|2|" });
    }
    program.push_str(predicate);
    for _ in 0..depth {
        program.push('|');
        program.push_str(predicate);
    }
    program
}

#[test]
fn check_programs_accept_the_exact_logical_budget_and_reject_one_under() {
    let schema = text_schema();
    let program = "AND|3|LIKE|0|3|M|S|La|OR|2|IN|0|2|Tone|N|EQ|0|T%00007C|NE|0|Tblocked";
    let metadata = CheckMetadata {
        table: "t",
        program,
        program_offset: 0,
    };
    let node_count = 6;
    let shape_depth = 2;
    let like_atoms = 3;
    let in_values = 2;
    let charged_text_bytes = "one".len() + "|".len() + "blocked".len();
    let exact = node_count * std::mem::size_of::<CheckProgramNode>()
        + shape_depth * std::mem::size_of::<ShapeFrame>()
        + like_atoms * std::mem::size_of::<LikeAtom>()
        + in_values * std::mem::size_of::<Value>()
        + charged_text_bytes;

    let mut exact_budget = ByteBudget::new(exact, Resource::StorageWorkingBytes);
    let (decoded, predicates) = decode_program(&schema, metadata, 0, usize::MAX, &mut exact_budget)
        .expect("the exact CHECK reconstruction budget is sufficient");
    assert_eq!(decoded.nodes().len(), node_count);
    assert_eq!(predicates, 5);
    assert!(
        exact_budget
            .charge(shape_depth * std::mem::size_of::<ShapeFrame>())
            .is_ok(),
        "completed CHECK reconstruction releases its shape stack"
    );
    assert!(matches!(
        exact_budget.charge(1),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit,
        }) if limit == exact
    ));

    let error = decode_program(
        &schema,
        CheckMetadata {
            table: "t",
            program,
            program_offset: 0,
        },
        0,
        usize::MAX,
        &mut ByteBudget::new(exact - 1, Resource::StorageWorkingBytes),
    )
    .expect_err("one byte below the CHECK reconstruction budget must fail");
    assert!(matches!(
        error,
        Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit,
        } if limit == exact - 1
    ));
}

#[test]
fn retained_check_programs_charge_logical_descriptors_not_vector_capacity() {
    let descriptor_bytes = std::mem::size_of::<CheckProgram>();
    let exact = 3 * descriptor_bytes;
    let mut checks = Vec::new();
    let mut exact_budget = ByteBudget::new(exact, Resource::StorageWorkingBytes);
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
    let mut one_under = ByteBudget::new(one_under_limit, Resource::StorageWorkingBytes);
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
                &mut ByteBudget::new(0, Resource::StorageWorkingBytes),
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
            &mut ByteBudget::new(0, Resource::StorageWorkingBytes),
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
        &mut ByteBudget::new(0, Resource::StorageWorkingBytes),
    )
    .expect_err("late malformed storage must not be masked by the working limit");

    assert!(matches!(
        error,
        Error::CorruptStorage { offset, message }
            if offset == 11 + program.find("BAD").expect("BAD opcode exists")
                && message == "unknown CHECK program opcode"
    ));
}

#[test]
fn earlier_noncanonical_nesting_precedes_later_structural_corruption_when_discoverable() {
    let schema = text_schema();
    let program = "AND|2|AND|2|EQ|0|Tone|EQ|0|Ttwo|BAD";
    let program_offset = 11;
    let nested_offset = program_offset
        + program
            .rfind("AND|2")
            .expect("nested associative opcode exists");
    let structural_offset =
        program_offset + program.find("BAD").expect("later malformed opcode exists");
    assert!(nested_offset < structural_offset);

    let metadata = || CheckMetadata {
        table: "t",
        program,
        program_offset,
    };
    assert!(matches!(
        decode_program(
            &schema,
            metadata(),
            0,
            usize::MAX,
            &mut ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes),
        ),
        Err(Error::CorruptStorage { offset, message })
            if offset == nested_offset
                && message == "CHECK program contains a noncanonical nested associative node"
    ));
    assert!(matches!(
        decode_program(
            &schema,
            metadata(),
            0,
            usize::MAX,
            &mut ByteBudget::new(0, Resource::StorageWorkingBytes),
        ),
        Err(Error::CorruptStorage { offset, message })
            if offset == structural_offset && message == "unknown CHECK program opcode"
    ));
}

#[test]
fn canonical_nesting_diagnostics_require_the_bounded_shape_stack() {
    let schema = text_schema();
    let program = "AND|2|AND|2|EQ|0|Tone|EQ|0|Ttwo|EQ|0|Tthree";
    let metadata = || CheckMetadata {
        table: "t",
        program,
        program_offset: 7,
    };

    assert!(matches!(
        decode_program(
            &schema,
            metadata(),
            0,
            usize::MAX,
            &mut ByteBudget::new(0, Resource::StorageWorkingBytes),
        ),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit: 0,
        })
    ));

    let nested_offset = 7 + program
        .rfind("AND|2")
        .expect("nested associative opcode exists");
    assert!(matches!(
        decode_program(
            &schema,
            metadata(),
            0,
            usize::MAX,
            &mut ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes),
        ),
        Err(Error::CorruptStorage { offset, message })
            if offset == nested_offset
                && message == "CHECK program contains a noncanonical nested associative node"
    ));
}

#[test]
fn deep_check_reconstruction_uses_the_exact_logical_peak() {
    const DEPTH: usize = 4_096;

    let schema = integer_schema();
    let program = alternating_binary_program(DEPTH, "EQ|0|I1");
    let node_count = 2 * DEPTH + 1;
    let predicate_count = DEPTH + 1;
    let retained_node_bytes = node_count * std::mem::size_of::<CheckProgramNode>();
    let shape_bytes = DEPTH * std::mem::size_of::<ShapeFrame>();
    let exact = retained_node_bytes + shape_bytes;

    let mut exact_budget = ByteBudget::new(exact, Resource::StorageWorkingBytes);
    let (decoded, predicates) = decode_program(
        &schema,
        CheckMetadata {
            table: "t",
            program: &program,
            program_offset: 0,
        },
        0,
        usize::MAX,
        &mut exact_budget,
    )
    .expect("the exact deep reconstruction budget is sufficient");
    assert_eq!(decoded.nodes().len(), node_count);
    assert_eq!(predicates, predicate_count);
    assert!(
        exact_budget.charge(shape_bytes).is_ok(),
        "successful reconstruction releases its full shape stack"
    );
    assert!(matches!(
        exact_budget.charge(1),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit,
        }) if limit == exact
    ));

    assert!(matches!(
        decode_program(
            &schema,
            CheckMetadata {
                table: "t",
                program: &program,
                program_offset: 0,
            },
            0,
            usize::MAX,
            &mut ByteBudget::new(exact - 1, Resource::StorageWorkingBytes),
        ),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit,
        }) if limit == exact - 1
    ));
}

#[test]
fn shape_prevalidation_is_linear_and_releases_partial_budget_on_failure() {
    const DEPTH: usize = 4_096;

    let schema = text_schema();
    let program = alternating_binary_program(DEPTH, "EQ|0|Tone");

    let available_frames = 3;
    let limit = available_frames * std::mem::size_of::<ShapeFrame>();
    let mut budget = ByteBudget::new(limit, Resource::StorageWorkingBytes);
    let error = decode_program(
        &schema,
        CheckMetadata {
            table: "t",
            program: &program,
            program_offset: 0,
        },
        0,
        usize::MAX,
        &mut budget,
    )
    .expect_err("a deeper shape than the working budget permits must fail");
    assert!(matches!(
        error,
        Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit: error_limit,
        } if error_limit == limit
    ));
    assert!(
        budget.charge(limit).is_ok(),
        "failed shape prevalidation releases every charged frame"
    );
}
