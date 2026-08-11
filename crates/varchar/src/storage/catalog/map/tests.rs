use super::{CatalogMap, next_capacity};
use crate::storage::budget::WorkingBudget;
use crate::{Error, Resource};

#[test]
fn ascending_and_descending_insertions_remain_searchable() {
    for descending in [false, true] {
        let mut map = CatalogMap::new();
        let mut budget = WorkingBudget::new(usize::MAX);
        let keys = (0..1_024).map(|index| format!("table_{index:04}"));
        let keys: Vec<_> = if descending {
            keys.rev().collect()
        } else {
            keys.collect()
        };

        for key in keys {
            let value = key.clone();
            map.insert_new(key, value, &mut budget, "reserving a test catalog")
                .expect("catalog insertion succeeds");
        }

        for index in 0..1_024 {
            let key = format!("table_{index:04}");
            assert_eq!(map.get(&key), Some(&key));
        }
        *map.get_mut("table_0512").expect("middle key exists") = String::from("updated");
        assert_eq!(map.get("table_0512").map(String::as_str), Some("updated"));
    }
}

#[test]
fn index_accounting_precedes_catalog_allocation() {
    let limit = std::mem::size_of::<usize>() * 3 - 1;
    let mut budget = WorkingBudget::new(limit);
    let mut map = CatalogMap::new();

    assert!(matches!(
        map.insert_new(
            String::from("items"),
            (),
            &mut budget,
            "reserving a test catalog"
        ),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit: actual,
        }) if actual == limit
    ));
    assert!(!map.contains_key("items"));
}

#[test]
fn exact_index_charge_is_independent_of_payload_size() {
    let index_bytes = std::mem::size_of::<usize>() * 3;
    let mut budget = WorkingBudget::new(index_bytes);
    let mut map = CatalogMap::new();

    map.insert_new(
        String::from("large"),
        [0_u8; 1_024],
        &mut budget,
        "reserving a test catalog",
    )
    .expect("one logical index entry exactly fits");
    assert!(map.contains_key("large"));
    assert!(matches!(
        map.insert_new(
            String::from("other"),
            [0_u8; 1_024],
            &mut budget,
            "reserving a test catalog"
        ),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit,
        }) if limit == index_bytes
    ));
    assert!(!map.contains_key("other"));
}

#[test]
fn failed_second_index_charge_preserves_the_first_entry() {
    let index_bytes = std::mem::size_of::<usize>() * 3;
    let limit = index_bytes * 2 - 1;
    let mut budget = WorkingBudget::new(limit);
    let mut map = CatalogMap::new();

    map.insert_new(
        String::from("first"),
        1,
        &mut budget,
        "reserving a test catalog",
    )
    .expect("first entry fits");
    assert!(matches!(
        map.insert_new(
            String::from("second"),
            2,
            &mut budget,
            "reserving a test catalog"
        ),
        Err(Error::ResourceLimit {
            resource: Resource::StorageWorkingBytes,
            limit: actual,
        }) if actual == limit
    ));
    assert_eq!(map.get("first"), Some(&1));
    assert!(!map.contains_key("second"));
}

#[test]
fn catalog_capacity_grows_geometrically() {
    assert_eq!(next_capacity(0), Some(1));
    assert_eq!(next_capacity(1), Some(2));
    assert_eq!(next_capacity(2), Some(4));
    assert_eq!(next_capacity(4), Some(8));
    assert_eq!(next_capacity(usize::MAX), None);
}
