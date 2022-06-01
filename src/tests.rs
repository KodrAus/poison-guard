use crate::{
    poison::PoisonGuard,
    Poison,
};
use std::{
    error::Error,
    io,
    panic,
};

mod poison_on_unwind;
mod poison_unless_recovered;

#[test]
fn poison_new_is_unpoisoned() {
    let poison = Poison::new(0);

    // A new guard is not poisoned
    assert!(!poison.is_poisoned());

    assert!(poison.get().is_ok());
}

#[test]
fn poison_new_catch_unwind() {
    let poison = Poison::new_catch_unwind(|| 0);

    // A non-panicked initializer is not poisoned
    assert!(!poison.is_poisoned());
}

#[test]
fn poison_new_catch_unwind_panic() {
    let poison: Poison<i32> = Poison::new_catch_unwind(|| panic!("explicit panic"));

    assert!(poison.is_poisoned());
    assert!(poison.get().is_err());
}

#[test]
fn poison_try_new_catch_unwind() {
    let poison = Poison::try_new_catch_unwind(|| Ok::<i32, SomeError>(0));

    assert!(!poison.is_poisoned());
    assert!(poison.get().is_ok());
}

#[test]
fn poison_try_new_catch_unwind_err() {
    let poison = Poison::try_new_catch_unwind(|| Err::<i32, SomeError>(some_err()));

    assert!(poison.is_poisoned());
    assert!(poison.get().is_err());
}

#[test]
fn poison_try_new_catch_unwind_panic() {
    fn try_new_catch_unwind() -> Result<i32, SomeError> {
        panic!("explicit panic")
    }

    let poison: Poison<i32> = Poison::try_new_catch_unwind(try_new_catch_unwind);

    assert!(poison.is_poisoned());
    assert!(poison.get().is_err());
}

#[test]
fn poison_get_unpoisoned() {
    let poison = Poison::new(0);

    assert_eq!(0, *poison.get().unwrap());
}

#[test]
fn poison_get_poisoned() {
    let mut poison = Poison::new(0);

    // Drop a guard without recovering, this will leave the value poisoned
    drop(Poison::unless_recovered(&mut poison).unwrap());

    assert!(poison.get().is_err());
}

#[test]
fn poison_recover_into_error() {
    fn try_with(v: &mut Poison<i32>) -> Result<(), Box<dyn Error + 'static>> {
        let guard = Poison::on_unwind(v)?;

        assert_eq!(42, *guard);

        Ok(())
    }

    assert!(try_with(&mut Poison::new(42)).is_ok());
    assert!(try_with(&mut Poison::new_catch_unwind(|| panic!("explicit panic"))).is_err());
}

type SomeError = io::Error;

fn some_err() -> SomeError {
    io::ErrorKind::Other.into()
}

fn unwind_through_guard<T>(guard: PoisonGuard<T>) {
    let _ = panic::catch_unwind(move || {
        let _ = &*guard;
        panic!("explicit panic");
    });
}

fn err_through_guard<T>(guard: PoisonGuard<T>) {
    drop(guard);
}

#[test]
#[cfg_attr(miri, ignore)]
fn ui() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass/*.rs");
}
