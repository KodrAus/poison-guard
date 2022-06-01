use crate::poison::*;

use std::{io, sync::Arc, thread};

use parking_lot::Mutex;

#[test]
fn poisoning_mutex() {
    let mutex = Mutex::new(Poison::new(42));

    let mut guard = Poison::on_unwind(mutex.lock()).unwrap();

    *guard = 43;
    drop(guard);

    let guard = mutex.lock();

    assert!(!guard.is_poisoned());

    let guard = Poison::on_unwind(guard).unwrap();
    assert_eq!(43, *guard);
    drop(guard);

    // Poison the guard without deadlocking the mutex
    let _ = Poison::err(
        Poison::on_unwind(mutex.lock()).unwrap(),
        io::Error::from(io::ErrorKind::Other),
    );

    let guard =
        Poison::on_unwind(mutex.lock()).unwrap_or_else(|guard| guard.recover_with(|v| *v = 42));

    assert_eq!(42, *guard);
}

#[test]
#[cfg_attr(miri, ignore)]
fn propagating_across_threads() {
    let mutex = Arc::new(Mutex::new(Poison::new(42)));

    let t = {
        let mutex = mutex.clone();
        thread::spawn(move || {
            let mut guard = mutex.lock().poison().unwrap();

            *guard += 1;

            panic!("explicit panic");
        })
    };

    assert!(t.join().is_err());

    assert!(mutex.lock().is_poisoned());
}
