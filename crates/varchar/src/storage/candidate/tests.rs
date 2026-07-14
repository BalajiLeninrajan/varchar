use super::Candidate;

#[test]
fn failed_splice_leaves_the_candidate_reusable() {
    let source = "V1;~R|t|I1;";
    let mut candidate = Candidate::new(source, source.len()).expect("source fits");

    assert!(
        candidate
            .splice(3..source.len(), "replacement is too large")
            .is_err()
    );
    assert_eq!(
        candidate.finish().expect("unchanged candidate fits"),
        source
    );
}
