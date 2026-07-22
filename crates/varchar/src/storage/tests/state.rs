use super::super::StorageState;
use super::super::validate::validate_and_catalog;

#[test]
fn state_keeps_each_blob_with_its_derived_catalog() {
    let blob = String::from("V2;~S|items|id:I:!;~R|items|I1;");
    let state = StorageState::load(blob.clone()).expect("fixture is valid");
    let reconstructed = validate_and_catalog(state.as_str()).expect("stored blob remains valid");

    assert_eq!(state.catalog(), &reconstructed);
    assert!(state.catalog().table("items").is_some());
    assert_eq!(state.into_string(), blob);
}
