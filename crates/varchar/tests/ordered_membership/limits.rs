use super::*;

fn bounded_members_blob() -> String {
    let mut database = Database::new();
    execute(
        &mut database,
        "CREATE TABLE bounded_members (id INTEGER NOT NULL, touched BOOLEAN NOT NULL)",
    );
    execute(
        &mut database,
        "INSERT INTO bounded_members VALUES (1, FALSE)",
    );
    execute(
        &mut database,
        "INSERT INTO bounded_members VALUES (2, FALSE)",
    );
    database.into_string()
}

#[test]
fn in_members_charge_one_predicate_unit_each_in_every_statement_kind() {
    let blob = bounded_members_blob();

    for sql in [
        "SELECT id FROM bounded_members WHERE id IN (1, 2, NULL)",
        "UPDATE bounded_members SET touched = TRUE WHERE id IN (1, 2, NULL)",
        "DELETE FROM bounded_members WHERE id IN (1, 2, NULL)",
    ] {
        let limits = Limits {
            max_predicates: 3,
            ..Limits::default()
        };
        let mut exact = Database::from_string_with_limits(blob.clone(), limits)
            .expect("fixture reloads at exact IN predicate limit");
        exact
            .execute(sql)
            .expect("three IN members fit limit three");

        let limits = Limits {
            max_predicates: 2,
            ..Limits::default()
        };
        let mut one_over = Database::from_string_with_limits(blob.clone(), limits)
            .expect("fixture reloads below IN predicate count");
        assert!(matches!(
            one_over.execute(sql),
            Err(Error::ResourceLimit {
                resource: Resource::WherePredicates,
                limit: 2,
            })
        ));
        assert_eq!(one_over.as_str(), blob);
    }
}

#[test]
fn logical_nodes_and_parentheses_add_no_units_to_in_member_counts() {
    let blob = bounded_members_blob();
    let sql = "SELECT id FROM bounded_members \
               WHERE (id IN (1, 2, NULL) OR touched = TRUE)";

    let limits = Limits {
        max_predicates: 4,
        ..Limits::default()
    };
    let mut exact = Database::from_string_with_limits(blob.clone(), limits)
        .expect("fixture reloads at exact mixed predicate limit");
    assert_eq!(
        rows(&mut exact, sql).into_rows(),
        vec![vec![Value::Integer(1)], vec![Value::Integer(2)]]
    );

    let limits = Limits {
        max_predicates: 3,
        ..Limits::default()
    };
    let mut one_over = Database::from_string_with_limits(blob.clone(), limits)
        .expect("fixture reloads below mixed predicate count");
    assert!(matches!(
        one_over.execute(sql),
        Err(Error::ResourceLimit {
            resource: Resource::WherePredicates,
            limit: 3,
        })
    ));
    assert_eq!(one_over.as_str(), blob);
}
