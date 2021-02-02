pub mod guard {
    use std::{
        mem::{self, MaybeUninit},
        ops, ptr,
    };

    pub struct InitCell<'a, T>(&'a mut Option<MaybeUninit<T>>);

    impl<'a, T> InitCell<'a, T> {
        pub fn get_mut(&mut self) -> &mut MaybeUninit<T> {
            self.0.as_mut().unwrap()
        }

        pub unsafe fn assume_init(self) -> T {
            self.0.take().unwrap().assume_init()
        }
    }

    impl<'a, T, const N: usize> InitCell<'a, [T; N]> {
        pub fn array_mut(&mut self) -> &mut [MaybeUninit<T>; N] {
            unsafe {
                mem::transmute::<&mut mem::MaybeUninit<[T; N]>, &mut [mem::MaybeUninit<T>; N]>(
                    self.get_mut(),
                )
            }
        }
    }

    pub struct UnwoundCell<T>(MaybeUninit<T>);

    impl<T> UnwoundCell<T> {
        pub fn into_inner(self) -> MaybeUninit<T> {
            self.0
        }
    }

    impl<T, const N: usize> UnwoundCell<[T; N]> {
        pub fn into_array(mut self) -> [MaybeUninit<T>; N] {
            unsafe {
                ptr::read(
                    &mut self.0 as *mut mem::MaybeUninit<[T; N]> as *mut [mem::MaybeUninit<T>; N],
                )
            }
        }
    }

    pub fn init<S, T>(
        mut state: S,
        init: impl FnOnce(&mut S, InitCell<T>) -> T,
        on_unwind: impl FnOnce(&mut S, UnwoundCell<T>),
    ) -> T {
        struct InitGuard<S, T, F>(*mut S, *mut Option<MaybeUninit<T>>, Option<F>)
        where
            F: FnOnce(&mut S, UnwoundCell<T>);

        impl<S, T, F> ops::Drop for InitGuard<S, T, F>
        where
            F: FnOnce(&mut S, UnwoundCell<T>),
        {
            fn drop(&mut self) {
                if let Some(unwound) = unsafe { &mut *self.1 }.take() {
                    let state = unsafe { &mut *self.0 };

                    (self.2.take().unwrap())(state, UnwoundCell(unwound));
                }
            }
        }

        let mut uninit = Some(MaybeUninit::<T>::uninit());
        let guard = InitGuard(
            &mut state as *mut S,
            &mut uninit as *mut Option<MaybeUninit<T>>,
            Some(on_unwind),
        );

        let init = init(&mut state, InitCell(&mut uninit));
        let _ = uninit.take();

        drop(guard);
        drop(state);

        init
    }
}

pub mod poison {
    use std::{fmt, ops, panic};

    /**
    A container that holds a potentially poisoned value.
    */
    // NOTE: This needs to live in `std`, not `core` because
    // it interacts with unwinding.
    pub struct Poison<T> {
        // TODO: This could be a `u8` to save space when combining with other flags
        poisoned: bool,
        recover_on_drop: bool,
        // TODO: Are there any opportunities to protect invalid state better?
        // TODO: Consider a `Result<T, PoisonPayload>`?
        // TODO: Where PoisonPayload may be PanicPayload or Error + Send + Sync + 'static?
        value: Option<T>,
    }

    impl<T> Poison<T> {
        /**
        Create a new `Poison<T>` with a valid inner value.
        */
        pub fn new(v: T) -> Self {
            Poison {
                poisoned: false,
                recover_on_drop: true,
                value: Some(v),
            }
        }

        pub fn catch_unwind(f: impl FnOnce() -> T) -> Self {
            match panic::catch_unwind(panic::AssertUnwindSafe(f)) {
                Ok(v) => Poison {
                    poisoned: false,
                    recover_on_drop: true,
                    value: Some(v),
                },
                Err(_) => Poison {
                    poisoned: true,
                    recover_on_drop: true,
                    value: None,
                },
            }
        }

        pub fn is_poisoned(&self) -> bool {
            self.poisoned || self.value.is_none()
        }

        /**
        Try poison the value and return a guard to it.

        If the guard is dropped normally the value will be unpoisoned.
        If the value is already poisoned it can be recovered.
        */
        pub fn poison<'a>(&'a mut self) -> Result<PoisonGuard<'a, T>, PoisonRecover<'a, T>> {
            Self::poison_target(self)
        }

        // NOTE: We can *almost* use arbitrary self types here (at least while they still work on inherent methods)
        // but can't because it breaks auto-ref
        pub fn poison_target<'a, Target>(
            mut target: Target,
        ) -> Result<PoisonGuard<'a, T, Target>, PoisonRecover<'a, T, Target>>
        where
            Target: ops::DerefMut<Target = Poison<T>> + 'a,
        {
            if target.is_poisoned() {
                Err(PoisonRecover {
                    target,
                    _marker: Default::default(),
                })
            } else {
                target.poisoned = true;
                let recover_on_drop = target.recover_on_drop;

                Ok(PoisonGuard {
                    target,
                    recover_on_drop,
                    _marker: Default::default(),
                })
            }
        }
    }

    /**
    A guard for a valid value.
    */
    pub struct PoisonGuard<'a, T, Target = &'a mut Poison<T>>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        target: Target,
        recover_on_drop: bool,
        _marker: std::marker::PhantomData<&'a mut T>,
    }

    impl<'a, T, Target> PoisonGuard<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        // TODO: Call this something more semantic? `poison_on_early_return`? `enter`/`exit`?
        pub fn recover_on_drop(&mut self, recover_on_drop: bool) {
            self.recover_on_drop = recover_on_drop;
        }
    }

    impl<'a, T, Target> ops::Drop for PoisonGuard<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        fn drop(&mut self) {
            // TODO: Also check if we're unwinding
            if self.recover_on_drop {
                self.target.poisoned = false;
            }
        }
    }

    impl<'a, T, Target> ops::Deref for PoisonGuard<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        type Target = T;

        fn deref(&self) -> &T {
            self.target.value.as_ref().expect("invalid poison")
        }
    }

    impl<'a, T, Target> ops::DerefMut for PoisonGuard<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        fn deref_mut(&mut self) -> &mut T {
            self.target.value.as_mut().expect("invalid poison")
        }
    }

    /**
    A guard for a poisoned value.
    */
    pub struct PoisonRecover<'a, T, Target = &'a mut Poison<T>>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        target: Target,
        _marker: std::marker::PhantomData<&'a mut T>,
    }

    impl<'a, T, Target> PoisonRecover<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        // TODO: Will this always just be the same function if recovery is possible?
        pub fn recover(mut self, f: impl FnOnce(Option<T>) -> T) -> PoisonGuard<'a, T, Target> {
            self.target.value = Some(f(self.target.value.take()));

            PoisonGuard {
                target: self.target,
                recover_on_drop: true,
                _marker: Default::default(),
            }
        }
    }

    impl<'a, T, Target> fmt::Debug for PoisonRecover<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.debug_struct("PoisonRecover").finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{guard::*, poison::*};
    use std::{mem, ops, ptr};

    #[test]
    fn it_works() {
        let mut v = Poison::new(42);

        let mut guard = v.poison().unwrap();

        assert_eq!(42, *guard);
        *guard += 1;
        drop(guard);

        // Dropping a guard shouldn't poison
        assert!(!v.is_poisoned());

        let guard = v.poison().unwrap();

        // The updated value should remain
        assert_eq!(43, *guard);
        drop(guard);

        // Poison the guard by forgetting it without dropping
        std::mem::forget(v.poison().unwrap());
        assert!(v.is_poisoned());

        // Unpoison the guard and decrement the value back down
        let guard = v.poison().unwrap_or_else(|guard| guard.recover(|_| 42));

        assert_eq!(42, *guard);
        drop(guard);

        // The value should no longer be poisoned
        assert!(!v.is_poisoned());
    }

    #[test]
    fn catch_unwind_produces_poisoned_guard() {
        let v = Poison::catch_unwind(|| panic!("lol"));

        assert!(v.is_poisoned());
    }

    mod mutex {
        use super::*;

        // An example wrapper for `parking_lot::Mutex`
        // This implements an inherent method on `MutexGuard<Poison<T>>` that shadows `Poison<T>`
        // We do this deliberately, knowing that the shadowed method is what we want
        // This is roughly what a new non-poisoning `Mutex` API would look like
        pub struct Mutex<T>(parking_lot::Mutex<T>);

        pub struct MutexGuard<'a, T>(parking_lot::MutexGuard<'a, T>);

        impl<T> Mutex<T> {
            pub fn new(value: T) -> Self {
                Mutex(parking_lot::Mutex::new(value))
            }

            pub fn lock(&self) -> MutexGuard<T> {
                MutexGuard(self.0.lock())
            }
        }

        impl<'a, T> MutexGuard<'a, Poison<T>> {
            /**
            Poison a locked value.
            */
            pub fn poison(
                self,
            ) -> Result<
                PoisonGuard<'a, T, MutexGuard<'a, Poison<T>>>,
                PoisonRecover<'a, T, MutexGuard<'a, Poison<T>>>,
            > {
                Poison::poison_target(self)
            }
        }

        impl<'a, T> ops::Deref for MutexGuard<'a, T> {
            type Target = T;

            fn deref(&self) -> &T {
                &*self.0
            }
        }

        impl<'a, T> ops::DerefMut for MutexGuard<'a, T> {
            fn deref_mut(&mut self) -> &mut T {
                &mut *self.0
            }
        }

        #[test]
        fn poisoning_mutex() {
            let mutex = Mutex::new(Poison::new(42));

            let mut guard = mutex.lock().poison().unwrap();

            *guard = 43;
            drop(guard);

            let guard = mutex.lock();

            assert!(!guard.is_poisoned());

            let guard = guard.poison().unwrap();
            assert_eq!(43, *guard);
            drop(guard);

            // Poison the guard without deadlocking the mutex
            let mut v = mutex.lock().poison().unwrap();
            v.recover_on_drop(false);
            drop(v);

            let guard = mutex
                .lock()
                .poison()
                .unwrap_or_else(|guard| guard.recover(|_| 42));

            assert_eq!(42, *guard);
        }
    }

    mod guard {
        use super::*;

        use std::sync::{Arc, Mutex};

        struct DropValue(Arc<Mutex<usize>>);

        impl ops::Drop for DropValue {
            fn drop(&mut self) {
                *self.0.lock().unwrap() += 1;
            }
        }

        #[test]
        fn init_guard_ok() {
            let arr: [u8; 16] = crate::guard::init(
                0usize,
                |i, mut uninit| {
                    let arr = uninit.array_mut();

                    while *i < 16 {
                        arr[*i] = mem::MaybeUninit::new(*i as u8);
                        *i += 1;
                    }

                    unsafe { uninit.assume_init() }
                },
                |i, unwound| {
                    let mut arr = unwound.into_array();

                    for i in 0..*i {
                        unsafe {
                            ptr::drop_in_place(arr[i].as_mut_ptr() as *mut u8);
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
            /*type ToInit = (usize, [mem::MaybeUninit<u8>; 16]);

            let (_, init) = InitArgs {
                to_init: (0, unsafe { mem::MaybeUninit::uninit().assume_init() }),
                on_init: |(i, arr): &mut ToInit| {
                    while *i < 16 {
                        arr[*i] = mem::MaybeUninit::new(*i as u8);
                        *i += 1;
                    }

                    Ok::<(), &'static str>(())
                },
                on_err_unwind: |(i, arr): &mut ToInit| {
                    for i in 0..*i {
                        unsafe {
                            ptr::drop_in_place(arr[i].as_mut_ptr() as *mut u8);
                        }
                    }
                },
            }
            .try_init()
            .unwrap();

            let arr = unsafe { mem::transmute::<[mem::MaybeUninit<u8>; 16], [u8; 16]>(init) };

            assert_eq!(
                [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
                arr
            );*/
            unimplemented!()
        }

        #[test]
        fn init_guard_panic() {
            let mut init_count = 0;
            let drop_count = Arc::new(Mutex::new(0));

            let p = Poison::catch_unwind(|| {
                let arr: [DropValue; 16] = crate::guard::init(
                    0usize,
                    |i, mut uninit| {
                        let arr = uninit.array_mut();

                        while *i < 16 {
                            arr[*i] = mem::MaybeUninit::new(DropValue(drop_count.clone()));
                            init_count += 1;

                            *i += 1;
                            if *i == 8 {
                                panic!("lol");
                            }
                        }

                        unsafe { uninit.assume_init() }
                    },
                    |i, unwound| {
                        let mut arr = unwound.into_array();

                        for i in 0..*i {
                            unsafe {
                                ptr::drop_in_place(arr[i].as_mut_ptr() as *mut DropValue);
                            }
                        }
                    },
                );

                arr
            });

            assert!(p.is_poisoned());

            assert!(init_count > 0);
            assert_eq!(init_count, *drop_count.lock().unwrap());
        }

        #[test]
        fn init_guard_try_err() {
            /*type ToInit = (usize, [mem::MaybeUninit<DropValue>; 16]);

            let mut init_count = 0;
            let drop_count = Arc::new(Mutex::new(0));

            let p = Poison::catch_unwind(|| {
                InitArgs {
                    to_init: (0, unsafe { mem::MaybeUninit::uninit().assume_init() }),
                    on_init: |(i, arr): &mut ToInit| {
                        while *i < 16 {
                            arr[*i] = mem::MaybeUninit::new(DropValue(drop_count.clone()));
                            init_count += 1;

                            *i += 1;
                            if *i == 8 {
                                return Err::<(), &'static str>("failed!");
                            }
                        }

                        Ok::<(), &'static str>(())
                    },
                    on_err_unwind: |(i, arr): &mut ToInit| {
                        for i in 0..*i {
                            unsafe {
                                ptr::drop_in_place(arr[i].as_mut_ptr() as *mut DropValue);
                            }
                        }
                    },
                }
                .try_init()
                .unwrap();
            });

            assert!(p.is_poisoned());

            assert!(init_count > 0);
            assert_eq!(init_count, *drop_count.lock().unwrap());*/
            unimplemented!()
        }

        #[test]
        fn init_guard_special_cleanup() {
            /*struct DeadLockOnDrop {
                ready: bool,
                lock: Arc<Mutex<usize>>,
            }

            impl DeadLockOnDrop {
                fn finalize(&mut self) {
                    match self.lock.clone().try_lock() {
                        Ok(mut guard) => self.finalize_sync(&mut *guard),
                        _ => panic!("deadlock!"),
                    }
                }

                fn finalize_sync(&mut self, guard: &mut usize) {
                    *guard += 1;
                }
            }

            impl ops::Drop for DeadLockOnDrop {
                fn drop(&mut self) {
                    if !self.ready {
                        self.finalize();
                    }
                }
            }

            let lock = Arc::new(Mutex::new(0));

            let p = Poison::catch_unwind(|| {
                // Acquire the lock here
                let mut guard = lock.lock().unwrap();

                // At this point, calling drop on `DeadLockOnDrop` would deadlock
                let (guard, value) = InitArgs {
                    to_init: (
                        &mut *guard,
                        DeadLockOnDrop {
                            ready: false,
                            lock: lock.clone(),
                        },
                    ),
                    on_init: |(guard, value): &mut (&mut usize, DeadLockOnDrop)| {
                        **guard += 1;
                        panic!("lol");
                        value.ready = true;
                    },
                    on_err_unwind: |(guard, value): &mut (&mut usize, DeadLockOnDrop)| {
                        value.finalize_sync(&mut *guard);
                    },
                }
                .init();

                drop(guard);

                value
            });*/
            unimplemented!()
        }

        #[test]
        fn init_guard_cheeky_mem_swap() {
            unimplemented!("what can we break with `mem::swap` (internally in `InitArgs`)?")
        }
    }
}
