use crate::poison::Poison;

use std::{error::Error, io, mem, panic};

#[test]
fn poison_size() {
    assert!(mem::size_of::<Poison<()>>() <= (mem::size_of::<usize>() * 2));
}

#[test]
fn unpoisoned_guard_can_access_value() {
    let mut v = Poison::new(42);

    let mut guard = Poison::unless_recovered(&mut v).unwrap();

    assert_eq!(42, *guard);
    *guard += 1;
    drop(guard);

    // Dropping a guard shouldn't poison
    assert!(!v.is_poisoned());
}

#[test]
fn guard_poisons_on_forget() {
    let mut p = Poison::new(42);

    let mut guard = Poison::on_unwind(&mut p).unwrap();

    // Forgetting a guard should poison
    mem::forget(guard);

    assert!(p.is_poisoned());
}

#[test]
fn guard_poisons_on_panic() {
    let mut v = Poison::new(42);

    let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let mut guard = Poison::on_unwind(&mut v).unwrap();

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
fn guard_recover() {
    let mut v = Poison::new(43);

    // Poison the guard by forgetting it without dropping
    mem::forget(v.as_mut().poison().unwrap());
    assert!(v.is_poisoned());

    // Unpoison the guard and decrement the value back down
    let guard = v
        .as_mut()
        .poison()
        .unwrap_or_else(|guard| guard.recover_with(|v| *v = 42));

    assert_eq!(42, *guard);
    drop(guard);

    // The value should no longer be poisoned
    assert!(!v.is_poisoned());
}

#[test]
fn guard_try_recover() {
    let mut v = Poison::new(43);

    // Poison the guard by forgetting it without dropping
    mem::forget(v.as_mut().poison().unwrap());
    assert!(v.is_poisoned());

    // Unpoison the guard and decrement the value back down
    let guard = v
        .as_mut()
        .poison()
        .or_else(|guard| {
            guard.try_recover_with(|v| {
                *v = 42;
                Ok::<(), SomeError>(())
            })
        })
        .unwrap();

    assert_eq!(42, *guard);
    drop(guard);

    // The value should no longer be poisoned
    assert!(!v.is_poisoned());
}

#[test]
fn catch_unwind_panic_poisons() {
    let v = Poison::new_catch_unwind(|| panic!("explicit panic"));

    assert!(v.is_poisoned());
}

#[test]
fn try_catch_unwind_err_poisons() {
    let v = Poison::try_new_catch_unwind(|| Err::<i32, SomeError>(some_err()));

    assert!(v.is_poisoned());
}

#[test]
fn scope_poisons_on_forget() {
    let mut p = Poison::new(42);

    let s = Poison::scope(p.as_mut().poison().unwrap());

    mem::forget(s);

    assert!(p.is_poisoned());
}

#[test]
fn scope_poisons_on_err() {
    let mut p = Poison::new(42);

    let mut s = Poison::scope(p.as_mut().poison().unwrap());

    let _ = s
        .try_catch_unwind(|_| Err::<(), SomeError>(some_err()))
        .unwrap_err();

    drop(s);

    assert!(p.is_poisoned());
}

#[test]
#[allow(unreachable_code)]
fn scope_poisons_on_panic() {
    let mut p = Poison::new(42);

    let mut s = Poison::scope(p.as_mut().poison().unwrap());

    let _ = s
        .try_catch_unwind(|_| {
            panic!("explicit panic");

            Ok::<(), SomeError>(())
        })
        .unwrap_err();

    drop(s);

    assert!(p.is_poisoned());
}

#[test]
fn scope_sync() {
    fn do_other_work() -> Result<(), SomeError> {
        Err(some_err())
    }

    fn do_work(p: &mut Poison<i32>) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        let mut s = Poison::scope(p.poison()?);

        s.try_catch_unwind(|v| {
            *v += 1;

            do_other_work()?;

            *v += 1;

            Ok::<(), SomeError>(())
        })?;

        let mut v2 = s.poison()?;

        *v2 += 1;

        Ok(())
    }

    let mut p = Poison::new(42);

    assert!(do_work(&mut p).is_err());
    assert!(p.is_poisoned());
}

#[tokio::test]
async fn scope_async() {
    async fn do_other_work() -> Result<(), SomeError> {
        Err(some_err())
    }

    async fn do_work(p: &mut Poison<i32>) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        let mut s = Poison::scope(p.poison()?);

        s.try_catch_unwind(|v| async move {
            *v += 1;

            do_other_work().await?;

            Ok::<(), SomeError>(())
        })
        .await?;

        let mut v = s.poison()?;

        *v += 1;

        Ok(())
    }

    let mut p = Poison::new(42);

    assert!(do_work(&mut p).await.is_err());
    assert!(p.is_poisoned());
}

#[test]
#[allow(unreachable_code)]
fn scope_escape() {
    let mut p = Poison::new(42);

    // We can escape a poison scope here
    let mut s = Poison::scope(p.as_mut().poison().unwrap());

    let mut v = None;
    let _ = s
        .try_catch_unwind(|g| {
            v = Some(g);
            panic!("explicit panic");

            *g += 1;

            Ok::<(), SomeError>(())
        })
        .unwrap_err();

    let v = v.unwrap();

    *v += 1;

    // `v` must go out of scope before we drop here
    drop(s);

    // In the end we still poison the value
    assert!(p.is_poisoned());

    // Scope escaping isn't ideal, but it's really not too different
    // from this, where we catch a panic and then "rethrow" it later
    // after continuing to access the value in the meantime
    let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let mut g = p.as_mut().poison().unwrap_or_else(|g| g.recover());
        let g = &mut *g;

        let mut v = None;
        let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            v = Some(g);
            panic!("explicit panic");

            *g += 1;

            Ok::<(), SomeError>(())
        }));

        let v = v.unwrap();

        *v += 1;

        let _ = r.unwrap();
    }));

    assert!(p.is_poisoned());
}

type SomeError = io::Error;

fn some_err() -> SomeError {
    io::ErrorKind::Other.into()
}
