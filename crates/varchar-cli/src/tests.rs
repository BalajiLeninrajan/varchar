use super::disambiguate_headers;

#[test]
fn unique_labels_stay_unqualified() {
    assert_eq!(
        disambiguate_headers([
            ("id", "users", "id"),
            ("name", "users", "name"),
            ("title", "posts", "title"),
        ]),
        ["id", "name", "title"]
    );
}

#[test]
fn repeated_projection_from_the_same_origin_stays_unqualified() {
    assert_eq!(
        disambiguate_headers([
            ("id", "users", "id"),
            ("id", "users", "id"),
            ("id", "users", "id"),
        ]),
        ["id", "id", "id"]
    );
}

#[test]
fn three_distinct_origins_with_the_same_label_are_qualified() {
    assert_eq!(
        disambiguate_headers([
            ("id", "users", "id"),
            ("id", "posts", "id"),
            ("id", "comments", "id"),
        ]),
        ["users.id", "posts.id", "comments.id"]
    );
}

#[test]
fn high_cardinality_metadata_does_not_require_rows() {
    let headers = disambiguate_headers(std::iter::repeat_n(("id", "items", "id"), 30_000));

    assert_eq!(headers.len(), 30_000);
    assert!(headers.iter().all(|header| header == "id"));
}
