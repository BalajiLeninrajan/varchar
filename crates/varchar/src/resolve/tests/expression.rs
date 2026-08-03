use super::{catalog, select_statement};
use crate::expression::Evaluator;
use crate::resolve::select;
use crate::{Error, Resource, Value};

#[test]
fn every_branch_is_resolved_before_runtime_short_circuiting() {
    let catalog = catalog("V2;~S|t|id:I:!|note:T:?;");
    let statement =
        select_statement(r"SELECT id FROM t WHERE id = 1 OR (note IS NULL AND note LIKE 'bad\q')");

    assert!(matches!(
        select(&catalog, &statement, 4, 4, usize::MAX),
        Err(Error::Type(ref message))
            if message == "LIKE pattern contains unsupported escape \\q"
    ));
}

#[test]
fn predicate_units_count_leaves_through_parentheses_and_operators() {
    let catalog = catalog("V2;~S|t|id:I:!|note:T:?;");
    let statement = select_statement(
        "SELECT id FROM t WHERE (id = 1 OR id = 2) AND (note IS NULL OR note = 'x')",
    );

    select(&catalog, &statement, 4, 4, usize::MAX).expect("exact predicate limit resolves");
    assert!(matches!(
        select(&catalog, &statement, 4, 3, usize::MAX),
        Err(Error::ResourceLimit {
            resource: Resource::WherePredicates,
            limit: 3,
        })
    ));
}

#[test]
fn deep_resolution_and_evaluation_use_explicit_stacks() {
    const DEPTH: usize = 1_500;
    let catalog = catalog("V2;~S|t|a:I:!;");
    let mut sql = String::from("SELECT a FROM t WHERE ");
    sql.push_str(&"(".repeat(DEPTH));
    sql.push_str("a = 1");
    for index in 0..DEPTH {
        if index % 2 == 0 {
            sql.push_str(" AND a = 1)");
        } else {
            sql.push_str(" OR a = 1)");
        }
    }
    let statement = select_statement(&sql);
    let resolved =
        select(&catalog, &statement, 1, DEPTH + 1, usize::MAX).expect("deep expression resolves");
    let program = resolved.where_clause.as_ref().expect("WHERE resolves");
    let mut evaluator = Evaluator::new(program, usize::MAX).expect("evaluation stack reserves");
    let row = [Value::Integer(1)];
    let rows = [row.as_slice()];
    assert!(
        evaluator
            .evaluate_where(program, &rows)
            .expect("deep expression evaluates")
    );
}
