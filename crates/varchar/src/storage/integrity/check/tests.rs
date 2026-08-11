use super::super::ValidationError;
use super::validate;
use crate::expression::CheckEvaluator;
use crate::storage::budget::WorkingBudget;
use crate::storage::validate::validate_and_catalog;
use crate::{Error, Resource, Value};

#[test]
fn check_rows_accept_the_exact_logical_budget_and_reject_one_under() {
    let blob = "V3;~S|t|value:T:?;~C|t|AND|2|LIKE|0|1|M|NE|0|Tblocked;~R|t|Tallowed;";
    let (_, catalog) =
        validate_and_catalog(blob, usize::MAX).expect("the CHECK row fixture is valid");
    let row_text_bytes = "allowed".len();
    let exact = std::mem::size_of::<Value>()
        + CheckEvaluator::working_bytes(1).expect("one evaluator frame can be sized")
        + row_text_bytes;

    let mut exact_budget = WorkingBudget::new(exact);
    assert!(
        validate(blob, &catalog, &mut exact_budget, usize::MAX).is_ok(),
        "the exact CHECK row workspace is sufficient"
    );
    assert!(
        exact_budget.charge(exact).is_ok(),
        "completed CHECK row validation releases its workspace"
    );

    assert!(matches!(
        validate(
            blob,
            &catalog,
            &mut WorkingBudget::new(exact - 1),
            usize::MAX,
        ),
        Err(ValidationError::Storage(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit,
        })) if limit == exact - 1
    ));
}
