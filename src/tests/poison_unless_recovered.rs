use crate::{
    tests::{
        err_through_guard,
        some_err,
        unwind_through_guard,
        SomeError,
    },
    Poison,
};

#[test]
fn guard_unless_recovered() {
    let mut poison = Poison::new(0);

    let mut guard = Poison::unless_recovered(&mut poison).unwrap();

    assert_eq!(0, *guard);

    *guard += 1;

    Poison::recover(guard);

    assert_eq!(1, *poison.get().unwrap());
}

#[test]
fn guard_unless_recovered_try_recover() {
    let mut poison = Poison::new(0);

    let mut guard = Poison::unless_recovered(&mut poison).unwrap();

    let _ = Poison::try_recover(
        (|| {
            *guard += 1;

            Ok::<(), SomeError>(())
        })(),
        guard,
    );

    assert_eq!(1, *poison.get().unwrap());
}

#[test]
fn guard_unless_recovered_poisons_on_try_recover_err() {
    let mut poison = Poison::new(0);

    let mut guard = Poison::unless_recovered(&mut poison).unwrap();

    let _ = Poison::try_recover(
        (|| {
            *guard += 1;

            Err::<(), SomeError>(some_err())
        })(),
        guard,
    );

    assert!(poison.is_poisoned());
}

#[test]
fn guard_unless_recovered_poisons_on_panic() {
    let mut poison = Poison::new(0);

    let guard = Poison::unless_recovered(&mut poison).unwrap();

    unwind_through_guard(guard);

    assert!(poison.is_poisoned());
}

#[test]
fn guard_unless_recovered_poisons_on_drop() {
    let mut poison = Poison::new(0);

    let guard = Poison::unless_recovered(&mut poison).unwrap();

    err_through_guard(guard);

    assert!(poison.is_poisoned());
}

#[test]
fn guard_unless_recovered_recover_unless_recovered() {
    let mut poison = Poison::new(0);

    unwind_through_guard(Poison::unless_recovered(&mut poison).unwrap());

    // Guards poisoned through an unwind can be recovered
    let recover = Poison::unless_recovered(&mut poison).unwrap_err();

    let guard = recover.recover();

    assert_eq!(0, *guard);
}

#[test]
fn guard_unless_recovered_recover_on_unwind() {
    let mut poison = Poison::new(0);

    unwind_through_guard(Poison::unless_recovered(&mut poison).unwrap());

    // Guards poisoned through an unwind can be recovered through implicit guards
    let recover = Poison::on_unwind(&mut poison).unwrap_err();

    let guard = recover.recover();

    assert_eq!(0, *guard);
}

#[test]
fn guard_unless_recovered_recover_with() {
    let mut poison = Poison::new(0);

    unwind_through_guard(Poison::unless_recovered(&mut poison).unwrap());

    let recover = Poison::unless_recovered(&mut poison).unwrap_err();

    let guard = recover.recover_with(|i| *i += 1);

    assert_eq!(1, *guard);
}
