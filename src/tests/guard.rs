use crate::{guard::*, poison::Poison};

use std::{
    io,
    mem::MaybeUninit,
    ops, ptr,
    sync::{Arc, Mutex},
};

struct DropValue(Arc<Mutex<usize>>);

impl ops::Drop for DropValue {
    fn drop(&mut self) {
        *self.0.lock().unwrap() += 1;
    }
}

struct DeadLockOnDrop {
    ready: bool,
    finalized: bool,
    lock: Arc<Mutex<usize>>,
}

impl DeadLockOnDrop {
    fn finalize(&mut self) {
        if !self.finalized {
            match self.lock.clone().try_lock() {
                Ok(mut guard) => self.finalize_sync(&mut *guard),
                _ => panic!("deadlock!"),
            }
        }
    }

    fn finalize_sync(&mut self, guard: &mut usize) {
        if !self.finalized {
            self.finalized = true;
            *guard += 1;
        }
    }
}

impl ops::Drop for DeadLockOnDrop {
    fn drop(&mut self) {
        if !self.ready {
            self.finalize();
        }
    }
}

#[test]
fn init_guard_ok() {
    let arr: [u8; 16] = init_unwind_safe(
        0usize,
        |i, mut uninit| {
            for elem in uninit.array_mut() {
                *elem = MaybeUninit::new(*i as u8);
                *i += 1;
            }

            unsafe { uninit.assume_init() }
        },
        |i, unwound| {
            for elem in &mut unwound.into_array()[0..*i] {
                unsafe {
                    ptr::drop_in_place(elem.as_mut_ptr() as *mut u8);
                }
            }
        },
    );

    assert_eq!(
        [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        arr
    );
}

#[test]
fn init_guard_try_ok() {
    let arr: Result<[u8; 16], &'static str> = try_init_unwind_safe(
        0usize,
        |i, mut uninit| {
            for elem in uninit.array_mut() {
                *elem = MaybeUninit::new(*i as u8);
                *i += 1;
            }

            Ok(unsafe { uninit.assume_init() })
        },
        |i, err_unwound| {
            for elem in &mut err_unwound.into_array()[0..*i] {
                unsafe {
                    ptr::drop_in_place(elem.as_mut_ptr() as *mut u8);
                }
            }
        },
    );

    assert_eq!(
        [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        arr.unwrap()
    );
}

#[test]
fn init_guard_panic() {
    let mut init_count = 0;
    let drop_count = Arc::new(Mutex::new(0));

    let p = Poison::new_catch_unwind(|| {
        let arr: [DropValue; 16] = init_unwind_safe(
            0usize,
            |i, mut uninit| {
                for elem in uninit.array_mut() {
                    *elem = MaybeUninit::new(DropValue(drop_count.clone()));
                    init_count += 1;

                    *i += 1;
                    if *i == 8 {
                        panic!("explicit panic during initialization");
                    }
                }

                unsafe { uninit.assume_init() }
            },
            |i, unwound| {
                for elem in &mut unwound.into_array()[0..*i] {
                    unsafe {
                        ptr::drop_in_place(elem.as_mut_ptr() as *mut DropValue);
                    }
                }
            },
        );

        Some(arr)
    });

    assert!(p.is_poisoned());

    assert!(init_count > 0);
    assert_eq!(init_count, *drop_count.lock().unwrap());
}

#[test]
fn init_guard_try_err() {
    let mut init_count = 0;
    let drop_count = Arc::new(Mutex::new(0));

    let p = Poison::try_new_catch_unwind(|| {
        let arr: Result<[DropValue; 16], io::Error> = try_init_unwind_safe(
            0usize,
            |i, mut uninit| {
                for elem in uninit.array_mut() {
                    *elem = MaybeUninit::new(DropValue(drop_count.clone()));
                    init_count += 1;

                    *i += 1;
                    if *i == 8 {
                        return Err(io::ErrorKind::Other.into());
                    }
                }

                Ok(unsafe { uninit.assume_init() })
            },
            |i, unwound| {
                for elem in &mut unwound.into_array()[0..*i] {
                    unsafe {
                        ptr::drop_in_place(elem.as_mut_ptr() as *mut DropValue);
                    }
                }
            },
        );

        arr.map(Some)
    });

    assert!(p.is_poisoned());

    assert!(init_count > 0);
    assert_eq!(init_count, *drop_count.lock().unwrap());
}

#[test]
fn init_guard_special_cleanup_panic() {
    let lock = Arc::new(Mutex::new(0));

    let p = Poison::new_catch_unwind(|| {
        // Acquire the lock here
        let guard = lock.lock().unwrap();

        let v = init_unwind_safe(
            guard,
            |guard, uninit| {
                let mut value = uninit.init(DeadLockOnDrop {
                    ready: false,
                    finalized: false,
                    lock: lock.clone(),
                });

                **guard += 1;
                if **guard == 1 {
                    panic!("explicit panic during initialization");
                }

                value.ready = true;
                value
            },
            |guard, unwound| {
                // We initialized the value before panicking
                let mut value = unsafe { unwound.into_inner().assume_init() };
                value.finalize_sync(&mut *guard);
            },
        );

        Some(v)
    });

    assert!(p.is_poisoned());
}

#[test]
fn init_guard_try_panic_on_err() {
    let p = Poison::try_new_catch_unwind(|| {
        let arr: Result<[u8; 16], io::Error> = try_init_unwind_safe(
            0usize,
            |i, mut uninit| {
                for elem in uninit.array_mut() {
                    *elem = MaybeUninit::new(*i as u8);

                    *i += 1;
                    if *i == 8 {
                        return Err(io::ErrorKind::Other.into());
                    }
                }

                Ok(unsafe { uninit.assume_init() })
            },
            |_, _| {
                // We're not actually leaking here, but want to make sure this doesn't abort
                panic!("explicit panic causing a leak");
            },
        );

        arr.map(Some)
    });

    assert!(p.is_poisoned());
}
