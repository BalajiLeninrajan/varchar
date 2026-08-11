use super::*;

#[test]
fn excluded_expression_forms_keep_structured_public_errors() {
    let mut database = Database::new();
    let before = database.as_str().to_owned();
    for (sql, expected_feature, marker) in [
        ("SELECT * FROM t WHERE NOT a = 1", "unary NOT", "NOT"),
        (
            "SELECT * FROM t WHERE TRUE",
            "bare Boolean constants",
            "TRUE",
        ),
        (
            "SELECT * FROM t WHERE FALSE",
            "bare Boolean constants",
            "FALSE",
        ),
        (
            "SELECT * FROM t WHERE 1 = 1",
            "literal-to-literal predicates",
            "1",
        ),
        (
            "SELECT * FROM t WHERE NULL",
            "literal-to-literal predicates",
            "NULL",
        ),
        (
            "SELECT * FROM t WHERE a = b",
            "column-to-column WHERE predicates",
            "b",
        ),
    ] {
        let expected_start = sql.find(marker).expect("fixture contains error marker");
        let expected_end = expected_start + marker.len();
        match database.execute(sql) {
            Err(Error::Unsupported {
                feature,
                span_start,
                span_end,
            }) => {
                assert_eq!(feature, expected_feature, "feature for {sql:?}");
                assert_eq!(
                    (span_start, span_end),
                    (expected_start, expected_end),
                    "span for {sql:?}"
                );
            }
            other => panic!("expected exact Unsupported error for {sql:?}, got {other:?}"),
        }
        assert_eq!(database.as_str(), before);
    }
}
