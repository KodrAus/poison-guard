use crate::poison::Poison;

use std::{error::Error, io, mem, panic};

#[test]
fn poison_size() {
    assert!(mem::size_of::<Poison<()>>() <= (mem::size_of::<usize>() * 2));
}

#[test]
fn unpoisoned_guard_can_access_value() {
    let mut v = Poison::new(42);

    let mut guard = v.as_mut().poison().unwrap();

    assert_eq!(42, *guard);
    *guard += 1;
    drop(guard);

    // Dropping a guard shouldn't poison
    assert!(!v.is_poisoned());
}

#[test]
fn guard_recover() {
    let mut v = Poison::new(43);

    // Poison the guard by forgetting it without dropping
    std::mem::forget(v.as_mut().poison().unwrap());
    assert!(v.is_poisoned());

    // Unpoison the guard and decrement the value back down
    let guard = v
        .as_mut()
        .poison()
        .unwrap_or_else(|guard| guard.recover(|v| *v = 42));

    assert_eq!(42, *guard);
    drop(guard);

    // The value should no longer be poisoned
    assert!(!v.is_poisoned());
}

#[test]
fn guard_try_recover() {
    let mut v = Poison::new(43);

    // Poison the guard by forgetting it without dropping
    std::mem::forget(v.as_mut().poison().unwrap());
    assert!(v.is_poisoned());

    // Unpoison the guard and decrement the value back down
    let guard = v
        .as_mut()
        .poison()
        .or_else(|guard| {
            guard.try_recover(|v| {
                *v = 42;
                Ok::<(), io::Error>(())
            })
        })
        .unwrap();

    assert_eq!(42, *guard);
    drop(guard);

    // The value should no longer be poisoned
    assert!(!v.is_poisoned());
}

#[test]
fn catch_unwind_produces_poisoned_guard() {
    let v = Poison::new_catch_unwind(|| panic!("explicit panic"));

    assert!(v.is_poisoned());
}

#[test]
fn guard_poisons_on_panic() {
    let mut v = Poison::new(42);

    let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let mut guard = v.as_mut().poison().unwrap();

        *guard += 1;

        panic!("explicit panic");
    }));

    assert!(v.is_poisoned());
}

#[test]
fn convert_poisoned_guard_into_error() {
    fn try_with(v: &mut Poison<i32>) -> Result<(), Box<dyn Error + 'static>> {
        let guard = v.poison()?;

        assert_eq!(42, *guard);

        Ok(())
    }

    assert!(try_with(&mut Poison::new(42)).is_ok());
    assert!(try_with(&mut Poison::new_catch_unwind(|| panic!("explicit panic"))).is_err());
}

#[test]
fn try_with() {
    fn try_with(v: &mut Poison<i32>) -> Result<(), Box<dyn Error + 'static>> {
        Poison::try_with_catch_unwind(
            v.poison().or_else(|recover| {
                recover.try_recover(|guard| {
                    *guard = 0;

                    Ok::<(), io::Error>(())
                })
            })?,
            |v| {
                *v += 1;

                if *v > 10 {
                    return Err(io::ErrorKind::Other.into());
                }

                Ok::<(), io::Error>(())
            },
        )?;

        Ok(())
    }

    let mut v = Poison::new(9);

    assert!(try_with(&mut v).is_ok());
    assert!(try_with(&mut v).is_err());
    assert!(try_with(&mut v).is_ok());

    assert!(try_with(&mut Poison::new_catch_unwind(|| panic!("explicit panic"))).is_ok());
}
