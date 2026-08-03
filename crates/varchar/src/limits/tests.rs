use super::{
    ByteBudget, Limits, Resource, charged_growth_items, check_limit, storage_working_limit,
};
use crate::Error;

#[test]
fn defaults_cover_every_resource_bound() {
    assert_eq!(
        Limits::default(),
        Limits {
            max_database_bytes: 64 * 1024 * 1024,
            max_sql_bytes: 64 * 1024,
            max_predicates: 64,
            max_join_sources: 64,
            max_pattern_bytes: 8 * 1024 * 1024,
            max_query_working_bytes: 32 * 1024 * 1024,
            max_query_output_bytes: 32 * 1024 * 1024,
            max_join_steps: 1_000_000,
            regex_backtrack_limit: 1_000_000,
        }
    );
}

#[test]
fn storage_working_limit_is_private_multiple_with_saturation() {
    assert_eq!(storage_working_limit(7), 28);
    assert_eq!(storage_working_limit(usize::MAX), usize::MAX);
}

#[test]
fn check_limit_preserves_structured_resource_metadata() {
    assert!(check_limit(4, 4, Resource::JoinSteps).is_ok());

    assert!(matches!(
        check_limit(5, 4, Resource::JoinSteps),
        Err(Error::ResourceLimit {
            resource: Resource::JoinSteps,
            limit: 4,
        })
    ));
}

#[test]
fn resources_have_human_readable_names() {
    let cases = [
        (Resource::DatabaseBytes, "database bytes"),
        (Resource::StorageWorkingBytes, "storage working bytes"),
        (Resource::SqlBytes, "SQL bytes"),
        (Resource::WherePredicates, "WHERE predicates"),
        (Resource::CheckPredicates, "CHECK predicates"),
        (Resource::JoinSources, "JOIN sources"),
        (Resource::GeneratedRegexBytes, "generated regex bytes"),
        (Resource::QueryWorkingBytes, "query working bytes"),
        (Resource::QueryOutputBytes, "query output bytes"),
        (Resource::JoinSteps, "JOIN execution steps"),
        (Resource::RegexBacktracking, "regex backtracking steps"),
    ];

    for (resource, human_display) in cases {
        assert_eq!(resource.to_string(), human_display);
    }
}

#[test]
fn geometric_growth_charges_exactly_what_its_appends_report() {
    const ITEM_COUNT: usize = 1_000;

    let item_bytes = size_of::<usize>();
    let mut budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
    let mut values = Vec::new();
    let mut charged = 0;
    for value in 0..ITEM_COUNT {
        charged += budget
            .push_charged(&mut values, value, "growing a test vector")
            .expect("an unlimited budget always grows");
    }

    assert_eq!(values, (0..ITEM_COUNT).collect::<Vec<_>>());
    // What the appends reported is the whole charge, and therefore the whole of what a release
    // owes back. `Vec::capacity` is deliberately not the yardstick: an allocator may round a
    // `try_reserve_exact` request up, and a release measured off a rounded-up capacity would
    // hand back bytes the budget was never charged.
    assert_eq!(budget.used, charged);
    assert_eq!(charged, charged_growth_items(ITEM_COUNT) * item_bytes);
    // 0, 2, 3, 4, 6, 9, ... 1066: growing by half reserves 1066 items for 1000 appends, so
    // this pins the growth factor as well as the ledger.
    assert_eq!(charged, 1_066 * item_bytes);
    assert!(
        values.capacity() >= 1_066,
        "the allocator may hold more than the reservation charged for, never less"
    );
}

#[test]
fn geometric_growth_fails_with_the_budgeted_resource() {
    let item_bytes = size_of::<usize>();
    let limit = item_bytes * 3;
    let mut budget = ByteBudget::new(limit, Resource::StorageWorkingBytes);
    let mut values: Vec<usize> = Vec::new();
    let mut charged = 0;

    for value in 0..3 {
        charged += budget
            .push_charged(&mut values, value, "growing a test vector")
            .expect("the first three items fit the limit");
    }
    assert_eq!(charged, item_bytes * 3);
    assert!(matches!(
        budget.push_charged(&mut values, 3, "growing a test vector"),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit: actual,
        }) if actual == limit
    ));
    assert_eq!(
        budget.used, charged,
        "a refused growth leaves the budget agreeing with what the appends reported"
    );
}

#[test]
fn one_budget_reports_whichever_resource_it_was_built_for() {
    let mut output = ByteBudget::new(4, Resource::QueryOutputBytes);
    assert!(matches!(
        output.charge(5),
        Err(Error::ResourceLimit {
            resource: Resource::QueryOutputBytes,
            limit: 4,
        })
    ));

    let mut working = ByteBudget::for_database_limit(1);
    assert_eq!(
        working
            .charge_items::<u8>(4)
            .expect("four bytes fit a four-byte working limit"),
        4
    );
    assert!(matches!(
        working.charge(1),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit: 4,
        })
    ));
}

#[test]
fn a_failed_reservation_refunds_its_charge() {
    let mut budget = ByteBudget::new(usize::MAX, Resource::StorageWorkingBytes);
    let mut values: Vec<u64> = Vec::new();
    let charged = budget
        .reserve_exact(&mut values, 3, "reserving test slots")
        .expect("an unlimited budget reserves");
    assert_eq!((charged, budget.used), (24, 24));

    let overflowing = isize::MAX as usize / size_of::<u64>() + 1;
    assert!(matches!(
        budget.reserve_exact(&mut values, overflowing, "reserving overflowing test slots"),
        Err(Error::Allocation {
            operation: "reserving overflowing test slots"
        })
    ));
    assert_eq!(
        budget.used, charged,
        "a reservation that could not allocate refunds its own charge and no more"
    );

    budget.release(charged);
    assert_eq!(budget.used, 0);
}
