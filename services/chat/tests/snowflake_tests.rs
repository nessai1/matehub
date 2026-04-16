use matehub_chat::snowflake;

#[test]
fn ids_are_unique() {
    snowflake::init();
    let mut ids: Vec<i64> = (0..1000).map(|_| snowflake::next_id()).collect();
    let len_before = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), len_before, "all 1000 IDs must be unique");
}

#[test]
fn ids_are_monotonically_increasing() {
    snowflake::init();
    let mut prev = snowflake::next_id();
    for _ in 0..100 {
        let next = snowflake::next_id();
        assert!(next > prev, "IDs must increase: {prev} -> {next}");
        prev = next;
    }
}

#[test]
fn current_bucket_is_positive() {
    let b = snowflake::current_bucket();
    assert!(b > 0, "current bucket must be positive: {b}");
}

#[test]
fn bucket_is_stable() {
    let b1 = snowflake::current_bucket();
    let b2 = snowflake::current_bucket();
    assert_eq!(b1, b2, "bucket must be stable within milliseconds");
}
