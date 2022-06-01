use std::io;

use crate::poison::Poison;

use once_cell::sync::Lazy;

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

#[test]
fn poisoning_lazy_err() {
    static LAZY: Lazy<Poison<i32>> =
        Lazy::new(|| Poison::try_new_catch_unwind(|| Err::<i32, SomeError>(some_err())));

    assert_eq!(42, *LAZY.get().unwrap());
}

type SomeError = io::Error;

fn some_err() -> SomeError {
    io::ErrorKind::Other.into()
}
