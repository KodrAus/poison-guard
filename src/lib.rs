pub mod guard {
    /*!
    Panic-safe initialization and cleanup.

    ## Is this just `try`/`catch`?

    Absolutely not, where did you learn such words?
    Once an exception has been caught, `catch` gives you tools to determine how its propagated.
    You may choose to ignore, rethrow, or repackage it.
    These functions don't let you catch a panic that occurs, just run some extra code along the way.

    ## Is this just `try`/`finally` then?

    That's a little closer, but still not quite there.
    A `finally` block executes on both normal and exceptional paths, where the unwind closures here only execute after a panic.
    */

    use std::{
        cell::UnsafeCell,
        mem::{self, MaybeUninit},
        ops, ptr,
    };

    /**
    Attempt to initialize a value that may panic.

    The initialization function will be called to produce a value, `T`, from a `MaybeUninit<T>`.
    If the initialization function panics, then the unwind function will be called.
    This gives the caller a chance to clean up any partially initialized state and avoid leaks.

    The state value is shared between initialization and unwinding so it can be used to determine
    what was and wasn't initialized.

    If the unwind function panics then it may trigger an abort.
    */
    pub fn init_unwind_safe<S, T>(
        state: S,
        // TODO: Make this `FnOnce(S, MaybeUninitSlot)`
        init: impl for<'state, 'init> FnOnce(
            &'state mut S,
            MaybeUninitSlot<'init, T>,
        ) -> InitSlot<'init, T>,
        on_unwind: impl for<'state> FnOnce(&'state mut S, UnwoundSlot<T>),
    ) -> T {
        // The value to initialize and state are stored in `UnsafeCell`s
        // They're shared between the drop impl for a guard and the init closure
        // Only one of these sources can access the values at a time
        let uninit = UnsafeCell::new(Some(MaybeUninit::<T>::uninit()));
        let state = UnsafeCell::new(state);

        let guard = InitGuard(&state, &uninit, Some(on_unwind));

        // Run the initialization function
        let init = init(
            unsafe { &mut *state.get() },
            MaybeUninitSlot(unsafe { &mut *uninit.get() }),
        );

        // Drop the unwind guard
        // This happens in a specific order:
        // - First, ensure the uninitialized state is `None`, this prevents the `on_unwind` closure from running
        // - Next, drop the guard (which won't then do any work, but will drop the `on_unwind` closure)
        // - Finally, drop the state value after the guard has had a chance to access it

        let value = mem::ManuallyDrop::new(InitSlot::into_inner(init));

        drop(guard);
        drop(state);

        mem::ManuallyDrop::into_inner(value)
    }

    /**
    Attempt to initialize a value that may fail or panic.

    The initialization function will be called to try produce a value, `T`, from a `MaybeUninit<T>`.
    If the initialization function fails or panics, then the unwind function will be called.
    This gives the caller a chance to clean up any partially initialized state and avoid leaks.

    The state value is shared between initialization and unwinding so it can be used to determine
    what was and wasn't initialized.

    If the unwind function panics then it may trigger an abort.
    */
    pub fn try_init_unwind_safe<S, T, E>(
        state: S,
        // TODO: Make this `FnOnce(S, MaybeUninitSlot) -> Result<(), E>`
        try_init: impl for<'state, 'init> FnOnce(
            &'state mut S,
            MaybeUninitSlot<'init, T>,
        ) -> Result<InitSlot<'init, T>, E>,
        on_err_unwind: impl for<'state> FnOnce(&'state mut S, UnwoundSlot<T>),
    ) -> Result<T, E> {
        // The value to initialize and state are stored in `UnsafeCell`s
        // They're shared between the drop impl for a guard and the init closure
        // Only one of these sources can access the values at a time
        let uninit = UnsafeCell::new(Some(MaybeUninit::<T>::uninit()));
        let state = UnsafeCell::new(state);

        let guard = InitGuard(&state, &uninit, Some(on_err_unwind));

        // Run the initialization function
        match try_init(
            unsafe { &mut *state.get() },
            MaybeUninitSlot(unsafe { &mut *uninit.get() }),
        ) {
            Ok(init) => {
                // Drop the unwind guard
                // This happens in a specific order:
                // - First, ensure the uninitialized state is `None`, this prevents the `on_err_unwind` closure from running
                // - Next, drop the guard (which won't then do any work, but will drop the `on_err_unwind` closure)
                // - Finally, drop the state value after the guard has had a chance to access it

                let value = mem::ManuallyDrop::new(InitSlot::into_inner(init));

                drop(guard);
                drop(state);

                Ok(mem::ManuallyDrop::into_inner(value))
            }
            Err(e) => {
                // Drop the unwind guard
                // Since initialization failed this will execute the unwind closure before returning the error

                drop(guard);
                drop(state);

                Err(e)
            }
        }
    }

    /**
    A potentially uninitialized value.

    This type is a wrapper around a `MaybeUninit<T>`.
    */
    pub struct MaybeUninitSlot<'a, T>(&'a mut Option<MaybeUninit<T>>);

    /**
    An initialized value.

    This is the result of initializing a `MaybeUninitSlot`.
    */
    pub struct InitSlot<'a, T>(MaybeUninitSlot<'a, T>);

    impl<'a, T> InitSlot<'a, T> {
        fn into_inner(slot: Self) -> T {
            // SAFETY: An `InitSlot` can only be created from an initialized value
            unsafe { (slot.0).0.take().unwrap().assume_init() }
        }
    }

    impl<'a, T> ops::Deref for InitSlot<'a, T> {
        type Target = T;

        fn deref(&self) -> &T {
            unsafe { &*self.0.get().as_ptr() }
        }
    }

    impl<'a, T> ops::DerefMut for InitSlot<'a, T> {
        fn deref_mut(&mut self) -> &mut T {
            unsafe { &mut *self.0.get_mut().as_mut_ptr() }
        }
    }

    impl<'a, T> MaybeUninitSlot<'a, T> {
        fn get(&self) -> &MaybeUninit<T> {
            self.0.as_ref().unwrap()
        }

        /**
        Get a reference to the value to initialize.
        */
        pub fn get_mut(&mut self) -> &mut MaybeUninit<T> {
            self.0.as_mut().unwrap()
        }

        /**
        Initialize the value.

        Any previously initialized state will be overwritten without being dropped.
        */
        pub fn init(self, value: T) -> InitSlot<'a, T> {
            *self.0.as_mut().unwrap() = MaybeUninit::new(value);
            InitSlot(self)
        }

        /**
        Consider the value fully initialized.

        This has the same safety requirements as `MaybeUninit::assume_init`.
        */
        pub unsafe fn assume_init(self) -> InitSlot<'a, T> {
            // TODO: Should this actually mark us as initialized?
            InitSlot(self)
        }
    }

    impl<'a, T, const N: usize> MaybeUninitSlot<'a, [T; N]> {
        /**
        Get a reference to the value to initialize as an array.
        */
        pub fn array_mut(&mut self) -> &mut [MaybeUninit<T>; N] {
            unsafe {
                mem::transmute::<&mut mem::MaybeUninit<[T; N]>, &mut [mem::MaybeUninit<T>; N]>(
                    self.get_mut(),
                )
            }
        }
    }

    /**
    A potentially initialized value.

    This type is a wrapper around a `MaybeUninit<T>`.
    It's up to the caller to figure out how to drop any partially initialized state in the value.
    */
    pub struct UnwoundSlot<T>(MaybeUninit<T>);

    impl<T> UnwoundSlot<T> {
        /**
        Take the partially initialized value.
        */
        pub fn into_inner(self) -> MaybeUninit<T> {
            self.0
        }
    }

    impl<T, const N: usize> UnwoundSlot<[T; N]> {
        /**
        Take the partially initialized value as an array.
        */
        pub fn into_array(mut self) -> [MaybeUninit<T>; N] {
            unsafe {
                ptr::read(
                    &mut self.0 as *mut mem::MaybeUninit<[T; N]> as *mut [mem::MaybeUninit<T>; N],
                )
            }
        }
    }

    struct InitGuard<'a, S, T, F>(
        &'a UnsafeCell<S>,
        &'a UnsafeCell<Option<MaybeUninit<T>>>,
        Option<F>,
    )
    where
        F: FnOnce(&mut S, UnwoundSlot<T>);

    impl<'a, S, T, F> ops::Drop for InitGuard<'a, S, T, F>
    where
        F: FnOnce(&mut S, UnwoundSlot<T>),
    {
        fn drop(&mut self) {
            // SAFETY: This exclusive access to the inner value doesn't overlap a borrow given to the init closure
            // It's run in the drop impl of this guard _after_ the init closure has returned or unwound
            if let Some(unwound) = unsafe { &mut *self.1.get() }.take() {
                // SAFETY: This exclusive access to the state doesn't overlap a borrow given to the init closure
                let state = unsafe { &mut *self.0.get() };

                (self.2.take().unwrap())(state, UnwoundSlot(unwound));
            }
        }
    }

    // TODO: `drop_unwind_safe` and `try_drop_unwind_safe`
}

pub mod poison {
    /*!
    Panic-safe containers.
    */

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

        /**
        Try create a new `Poison<T>` with an initialization function that may panic.
        */
        pub fn catch_unwind(f: impl FnOnce() -> T) -> Self {
            // We're pretending the `UnwindSafe` and `RefUnwindSafe` traits don't exist
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

        /**
        Try create a new `Poison<T>` with an initialization function that may fail or panic.
        */
        pub fn try_catch_unwind<E>(f: impl FnOnce() -> Result<T, E>) -> Self {
            match panic::catch_unwind(panic::AssertUnwindSafe(f)) {
                Ok(Ok(v)) => Poison {
                    poisoned: false,
                    recover_on_drop: true,
                    value: Some(v),
                },
                // TODO: Can we actually capture the `E` somehow?
                _ => Poison {
                    poisoned: true,
                    recover_on_drop: true,
                    value: None,
                },
            }
        }

        /**
        Whether or not the `Poison<T>` is actually poisoned.
        */
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
        /**
        Try poison the value contained behind some target reference and return a guard to it.
        */
        pub fn poison_target<'a, Target>(
            mut target: Target,
        ) -> Result<PoisonGuard<'a, T, Target>, PoisonRecover<'a, T, Target>>
        where
            Target: ops::DerefMut<Target = Poison<T>> + 'a,
        {
            // TODO: Consider safety of adversarial `Deref` impls.
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

        struct DeadLockOnDrop {
            ready: bool,
            finalized: bool,
            lock: Arc<Mutex<usize>>,
        }

        impl DeadLockOnDrop {
            fn finalize(&mut self) {
                if !self.finalized {
                    self.finalized = true;

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
            let arr: Result<[u8; 16], &'static str> = try_init_unwind_safe(
                0usize,
                |i, mut uninit| {
                    let arr = uninit.array_mut();

                    while *i < 16 {
                        arr[*i] = mem::MaybeUninit::new(*i as u8);
                        *i += 1;
                    }

                    Ok(unsafe { uninit.assume_init() })
                },
                |i, err_unwound| {
                    let mut arr = err_unwound.into_array();

                    for i in 0..*i {
                        unsafe {
                            ptr::drop_in_place(arr[i].as_mut_ptr() as *mut u8);
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

            let p = Poison::catch_unwind(|| {
                let arr: [DropValue; 16] = init_unwind_safe(
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
            let mut init_count = 0;
            let drop_count = Arc::new(Mutex::new(0));

            let p = Poison::try_catch_unwind(|| {
                let arr: Result<[DropValue; 16], &'static str> = try_init_unwind_safe(
                    0usize,
                    |i, mut uninit| {
                        let arr = uninit.array_mut();

                        while *i < 16 {
                            arr[*i] = mem::MaybeUninit::new(DropValue(drop_count.clone()));
                            init_count += 1;

                            *i += 1;
                            if *i == 8 {
                                return Err("lol");
                            }
                        }

                        Ok(unsafe { uninit.assume_init() })
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
        fn init_guard_special_cleanup_panic() {
            let lock = Arc::new(Mutex::new(0));

            let p = Poison::catch_unwind(|| {
                // Acquire the lock here
                let guard = lock.lock().unwrap();

                init_unwind_safe(
                    guard,
                    |guard, mut uninit| {
                        *uninit.get_mut() = mem::MaybeUninit::new(DeadLockOnDrop {
                            ready: false,
                            finalized: false,
                            lock: lock.clone(),
                        });

                        let mut value = unsafe { uninit.assume_init() };
                        value.ready = true;

                        **guard += 1;

                        if **guard == 1 {
                            panic!("lol");
                        }

                        value
                    },
                    |guard, unwound| {
                        // We initialized the value before panicking
                        let mut value = unsafe { unwound.into_inner().assume_init() };
                        value.finalize_sync(&mut *guard);
                    },
                )
            });

            assert!(p.is_poisoned());
        }

        #[test]
        fn init_guard_cheeky_mem_swap() {
            unimplemented!("what can we break with `mem::swap` (internally in `InitArgs`)?")
        }

        #[test]
        fn init_guard_try_panic_on_err() {
            unimplemented!("make sure we don't call `Drop` on the initialized value if `unwind` closure panics")
        }
    }
}
