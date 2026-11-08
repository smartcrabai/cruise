use addition_fixture::add;

#[test]
fn checked_addition() {
    assert_eq!(add(2, 3), Some(5));
    assert_eq!(add(-2, 1), Some(-1));
    assert_eq!(add(0, 0), Some(0));
    assert_eq!(add(i32::MAX, 1), None);
}
