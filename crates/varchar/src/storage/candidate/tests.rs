use super::StorageState;

#[test]
fn failed_splice_leaves_the_candidate_reusable() {
    let state = StorageState::load(String::from("V2;~S|t|id:I:!;~R|t|I1;"), usize::MAX)
        .expect("source is valid");
    let source = state.as_str();
    let max_bytes = 256;
    let mut candidate = state.candidate(max_bytes).expect("source fits");
    let oversized = "x".repeat(max_bytes + 1);

    assert!(candidate.splice(3..source.len(), &oversized).is_err());
    assert_eq!(
        candidate
            .finish()
            .expect("unchanged candidate fits")
            .as_str(),
        source
    );
}

#[test]
fn finish_rejects_an_invalid_replacement_state() {
    let state = StorageState::empty();
    let mut candidate = state.candidate(64).expect("empty state fits");
    candidate
        .splice(state.as_str().len()..state.as_str().len(), "garbage")
        .expect("unvalidated edit fits");

    assert!(matches!(
        candidate.finish(),
        Err(crate::Error::CorruptStorage { .. })
    ));
    assert_eq!(state.as_str(), "V2;");
}
