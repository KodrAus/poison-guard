use crate::{
    tests::unwind_through_guard,
    Poison,
};

#[test]
fn guard_on_unwind() {
    let mut poison = Poison::new(0);

    let mut guard = Poison::on_unwind(&mut poison).unwrap();

    assert_eq!(0, *guard);

    *guard += 1;

    drop(guard);

    assert_eq!(1, *poison.get().unwrap());
}

#[test]
fn guard_on_unwind_poisons_on_panic() {
    let mut poison = Poison::new(0);

    let guard = Poison::on_unwind(&mut poison).unwrap();

    unwind_through_guard(guard);

    assert!(poison.is_poisoned());
}

#[test]
fn guard_on_unwind_recover_on_unwind() {
    let mut poison = Poison::new(0);

    unwind_through_guard(Poison::on_unwind(&mut poison).unwrap());

    // Guards poisoned through an unwind can be recovered
    let recover = Poison::on_unwind(&mut poison).unwrap_err();

    let guard = recover.recover();

    assert_eq!(0, *guard);
}

#[test]
fn guard_on_unwind_recover_unless_recovered() {
    let mut poison = Poison::new(0);

    unwind_through_guard(Poison::on_unwind(&mut poison).unwrap());

    // Guards poisoned through an unwind can be recovered through explicit guards
    let recover = Poison::unless_recovered(&mut poison).unwrap_err();

    let guard = recover.recover();

    assert_eq!(0, *guard);
}

#[test]
fn guard_on_unwind_recover_with() {
    let mut poison = Poison::new(0);

    unwind_through_guard(Poison::on_unwind(&mut poison).unwrap());

    let recover = Poison::on_unwind(&mut poison).unwrap_err();

    let guard = recover.recover_with(|i| *i += 1);

    assert_eq!(1, *guard);
}
