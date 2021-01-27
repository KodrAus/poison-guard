pub mod poison {
    use std::{fmt, ops, panic};

    /**
    A container that holds a potentially poisoned value.
    */
    pub struct Poison<T> {
        // TODO: This could be a `u8` to save space
        poisoned: bool,
        // TODO: Are there any opportunities to protect invalid state better?
        value: T,
    }

    impl Poison<()> {
        pub fn catch_unwind(f: impl FnOnce()) -> Self {
            match panic::catch_unwind(panic::AssertUnwindSafe(f)) {
                Ok(()) => Poison {
                    poisoned: false,
                    value: (),
                },
                Err(_) => Poison {
                    poisoned: true,
                    value: (),
                },
            }
        }
    }

    impl<T> Poison<T> {
        /**
        Create a new `Poison<T>` with a valid inner value.
        */
        pub fn new(value: T) -> Self {
            Poison {
                poisoned: false,
                value,
            }
        }

        pub fn is_poisoned(&self) -> bool {
            self.poisoned
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
            if target.poisoned {
                Err(PoisonRecover {
                    target,
                    _marker: Default::default(),
                })
            } else {
                target.poisoned = true;

                Ok(PoisonGuard {
                    target,
                    unpoison_on_drop: true,
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
        unpoison_on_drop: bool,
        _marker: std::marker::PhantomData<&'a mut T>,
    }

    impl<'a, T, Target> PoisonGuard<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        // TODO: Call this something more semantic? `poison_on_early_return`? `enter`/`exit`?
        pub fn unpoison_on_drop(&mut self, unpoison_on_drop: bool) {
            self.unpoison_on_drop = unpoison_on_drop;
        }
    }

    impl<'a, T, Target> ops::Drop for PoisonGuard<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        fn drop(&mut self) {
            // TODO: Also check if we're unwinding
            if self.unpoison_on_drop {
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
            &self.target.value
        }
    }

    impl<'a, T, Target> ops::DerefMut for PoisonGuard<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        fn deref_mut(&mut self) -> &mut T {
            &mut self.target.value
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
        pub fn unpoison(mut self, f: impl FnOnce(&mut T)) -> PoisonGuard<'a, T, Target> {
            f(&mut self.target.value);

            PoisonGuard {
                target: self.target,
                unpoison_on_drop: true,
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
    use crate::poison::*;
    use std::ops;

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
        let guard = v
            .poison()
            .unwrap_or_else(|guard| guard.unpoison(|v| *v = 42));

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
        v.unpoison_on_drop(false);
        drop(v);

        let guard = mutex
            .lock()
            .poison()
            .unwrap_or_else(|guard| guard.unpoison(|v| *v = 42));

        assert_eq!(42, *guard);
    }
}
