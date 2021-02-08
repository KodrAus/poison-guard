use crate::poison::Poison;

use std::{error::Error, io, mem, panic, ops::Try};

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
    std::mem::forget(v.as_mut().poison().unwrap());
    assert!(v.is_poisoned());

    // Unpoison the guard and decrement the value back down
    let guard = v
        .as_mut()
        .poison()
        .or_else(|guard| {
            guard.try_recover_with(|v| {
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
fn scope_sync() {
    fn do_other_work() -> Result<(), io::Error> {
        Err(io::Error::from(io::ErrorKind::Other))
    }

    fn do_work(p: &mut Poison<i32>) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        let mut s = Poison::scope(p.poison()?);

        let v1 = s.try_catch_unwind(|v| {
            *v += 1;

            do_other_work()?;

            *v += 1;

            Ok::<(), io::Error>(())
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
    async fn do_other_work() -> Result<(), io::Error> {
        Err(io::Error::from(io::ErrorKind::Other))
    }

    async fn do_work(p: &mut Poison<i32>) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        let mut s = Poison::scope(p.poison()?);

        s.try_catch_unwind(|v| async move {
            *v += 1;

            do_other_work().await?;

            Ok::<(), io::Error>(())
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
fn scope_escape() {
    let mut p = Poison::new(42);

    // We can escape a poison scope here
    let mut s = Poison::scope(p.as_mut().poison().unwrap());

    let mut v = None;
    let _ = s.try_catch_unwind(|g| {
        v = Some(g);
        panic!("explicit panic");

        Ok::<(), io::Error>(())
    }).into_result().unwrap_err();

    let mut v = v.unwrap();

    *v += 1;

    drop(s);

    // In the end we still poison the value
    assert!(p.is_poisoned());

    // But that's pretty much the same as doing this
    let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let mut g = p.as_mut().poison().unwrap_or_else(|g| g.recover());

        let mut v = None;
        let r = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            v = Some(g);
            panic!("explicit panic");

            Ok::<(), io::Error>(())
        }));

        let mut v = v.unwrap();

        *v += 1;

        let _ = r.unwrap();
    }));

    assert!(p.is_poisoned());
}
