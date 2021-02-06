use crate::poison::Poison;

use std::lazy::SyncLazy as Lazy;

#[test]
fn poisoning_lazy_ok() {
    static LAZY: Lazy<Poison<i32>> = Lazy::new(|| Poison::new_catch_unwind(|| 42));

    assert_eq!(42, *LAZY.get().unwrap());
}

#[test]
fn poisoning_lazy_panic() {
    static LAZY: Lazy<Poison<i32>> =
        Lazy::new(|| Poison::new_catch_unwind(|| panic!("explicit panic during initialization")));

    assert!(LAZY.is_poisoned());
}
