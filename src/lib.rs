#![feature(backtrace, once_cell)]

// NOTE: Could be `no_std`.
pub mod guard {
    /*!
    Unwind-safe initialization and cleanup.

    ## Is this just `try`/`catch`?

    Absolutely not, where did you learn such words?
    Once an exception has been caught, `catch` gives you tools to determine how its propagated.
    You may choose to ignore, rethrow, or repackage it.
    These functions don't let you catch an unwind that occurs, just run some extra code along the way.

    ## Is this just `try`/`finally` then?

    That's a little closer, but still not quite there.
    A `finally` block executes on both normal and exceptional paths, where the unwind closures here only execute after an unwind.
    */

    use std::{
        cell::UnsafeCell,
        mem::{self, MaybeUninit},
        ops, ptr,
    };

    /**
    Attempt to initialize a value that may unwind.

    The initialization function will be called to produce a value, `T`, from a `MaybeUninit<T>`.
    If the initialization function unwinds, then the unwind function will be called.
    This gives the caller a chance to clean up any partially initialized state and avoid leaks.

    The state value is shared between initialization and unwinding so it can be used to determine
    what was and wasn't initialized.

    If the unwind function panics then it may trigger an abort.

    `init_unwind_safe` guarantees that `Drop` won't be called on `T` (barring any use of `mem::{take, swap, replace}`)
    if the `on_unwind` closure executes.
    */
    pub fn init_unwind_safe<S, T>(
        state: S,
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
            // SAFETY: These exclusive accesses to the inner value and state doesn't overlap a borrow given to the unwind closure
            // These borrows expire _before_ the unwind closure gets a chance to run
            unsafe { &mut *state.get() },
            MaybeUninitSlot(unsafe { &mut *uninit.get() }),
        );

        // Ensure the guard hasn't been swapped
        // TODO: This check is a reminder to think about implications of `mem::swap` and friends
        // This _shouldn't_ be possible since we have an arbitrarily small invariant lifetime, but is worth trying to break
        assert_eq!(&uninit as *const _, (init.0).0 as *mut _ as *const _);

        // Drop the unwind guard
        // This happens in a specific order:
        // - First, ensure the uninitialized state is `None`, this prevents the `on_unwind` closure from running
        // - Next, drop the guard (which won't then do any work, but will drop the `on_unwind` closure)
        // - Finally, drop the state value after the guard has had a chance to access it

        let value = InitSlot::into_inner(init);

        // Dropping the guard here will never panic, but dropping the state might
        // If that happens we unwind regularly, since the value is fully initialized
        drop(guard);
        drop(state);

        value
    }

    /**
    Attempt to initialize a value that may fail or unwind.

    The initialization function will be called to try produce a value, `T`, from a `MaybeUninit<T>`.
    If the initialization function fails or unwinds, then the unwind function will be called.
    This gives the caller a chance to clean up any partially initialized state and avoid leaks.

    The state value is shared between initialization and unwinding so it can be used to determine
    what was and wasn't initialized.

    If the unwind function panics then it may trigger an abort.

    `try_init_unwind_safe` guarantees that `Drop` won't be called on `T` (barring any use of `mem::{take, swap, replace}`)
    if the `on_err_unwind` closure executes.
    */
    pub fn try_init_unwind_safe<S, T, E>(
        state: S,
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
            // SAFETY: These exclusive accesses to the inner value and state doesn't overlap a borrow given to the unwind closure
            // These borrows expire _before_ the unwind closure gets a chance to run
            unsafe { &mut *state.get() },
            MaybeUninitSlot(unsafe { &mut *uninit.get() }),
        ) {
            Ok(init) => {
                // Drop the unwind guard
                // This happens in a specific order:
                // - First, ensure the uninitialized state is `None`, this prevents the `on_err_unwind` closure from running
                // - Next, drop the guard (which won't then do any work, but will drop the `on_err_unwind` closure)
                // - Finally, drop the state value after the guard has had a chance to access it

                // Ensure the guard hasn't been swapped
                // TODO: This check is a reminder to think about implications of `mem::swap` and friends
                // This _shouldn't_ be possible since we have an arbitrarily small invariant lifetime, but is worth trying to break
                assert_eq!(&uninit as *const _, (init.0).0 as *mut _ as *const _);

                let value = InitSlot::into_inner(init);

                // Dropping the guard here will never panic, but dropping the state might
                // If that happens we unwind regularly, since the value is fully initialized
                drop(guard);
                drop(state);

                Ok(value)
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
            // SAFETY: An `InitSlot` can only be created from an initialized value
            unsafe { &*self.0.get().as_ptr() }
        }
    }

    impl<'a, T> ops::DerefMut for InitSlot<'a, T> {
        fn deref_mut(&mut self) -> &mut T {
            // SAFETY: An `InitSlot` can only be created from an initialized value
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
            InitSlot(self)
        }
    }

    impl<'a, T, const N: usize> MaybeUninitSlot<'a, [T; N]> {
        /**
        Get a reference to the value to initialize as an array.
        */
        pub fn array_mut(&mut self) -> &mut [MaybeUninit<T>; N] {
            // SAFETY: `MaybeUninit<T>` has the same layout as `T`
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
            // SAFETY: `MaybeUninit<T>` has the same layout as `T`
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

// NOTE: Can't be `no_std` because of `catch_unwind`.
pub mod poison {
    /*!
    Unwind-safe containers.
    */

    use std::{
        any::Any, backtrace::Backtrace, borrow::Cow, error::Error, fmt, mem, ops, panic, sync::Arc,
        thread,
    };

    /**
    A container that holds a potentially poisoned value.
    */
    pub struct Poison<T> {
        value: T,
        poisoned: PoisonState,
    }

    impl<T> Poison<T> {
        /**
        Create a new `Poison<T>` with a valid inner value.
        */
        pub fn new(v: T) -> Self {
            Poison {
                value: v,
                poisoned: PoisonState::unpoisoned(),
            }
        }

        /**
        Try create a new `Poison<T>` with an initialization function that may unwind.

        If initialization does unwind then the panic payload will be caught and stashed inside the `Poison<T>`.
        Any attempt to access the poisoned value will instead return this payload unless the `Poison<T>` is restored.
        */
        pub fn catch_unwind(f: impl FnOnce() -> T) -> Self
        where
            T: Default,
        {
            // We're pretending the `UnwindSafe` and `RefUnwindSafe` traits don't exist
            match panic::catch_unwind(panic::AssertUnwindSafe(f)) {
                Ok(v) => Poison {
                    value: v,
                    poisoned: PoisonState::unpoisoned(),
                },
                Err(panic) => Poison {
                    value: Default::default(),
                    poisoned: PoisonState::from_panic(Some(panic)),
                },
            }
        }

        /**
        Try create a new `Poison<T>` with an initialization function that may fail or unwind.

        If initialization does unwind then the error or panic payload will be caught and stashed inside the `Poison<T>`.
        Any attempt to access the poisoned value will instead return this payload unless the `Poison<T>` is restored.
        */
        pub fn try_catch_unwind<E>(f: impl FnOnce() -> Result<T, E>) -> Self
        where
            T: Default,
            E: Error + Send + Sync + 'static,
        {
            match panic::catch_unwind(panic::AssertUnwindSafe(f)) {
                Ok(Ok(v)) => Poison {
                    value: v,
                    poisoned: PoisonState::unpoisoned(),
                },
                Ok(Err(e)) => Poison {
                    value: Default::default(),
                    poisoned: PoisonState::from_err(Some(Box::new(e))),
                },
                Err(panic) => Poison {
                    value: Default::default(),
                    poisoned: PoisonState::from_panic(Some(panic)),
                },
            }
        }

        /**
        Whether or not the `Poison<T>` is actually poisoned.
        */
        pub fn is_poisoned(&self) -> bool {
            self.poisoned.is_poisoned()
        }

        /**
        Try get the inner value.

        This will return `Err` if the value is poisoned.
        */
        pub fn get(&self) -> Result<&T, &(dyn Error + 'static)> {
            if let PoisonState::Err(ref err) = self.poisoned {
                Err(&*err.source)
            } else {
                Ok(&self.value)
            }
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
        // NOTE: This lets us do something like wrap a guard internally without having to deal with mutable references.
        // NOTE: An alternative to this could be scoped poisoning in closures. These aren't always easy to work with though.
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
                target.poisoned = PoisonState::sentinel();

                Ok(PoisonGuard {
                    target,
                    recover_on_drop: true,
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
        pub fn recover_on_drop(guard: &mut Self, recover_on_drop: bool) {
            guard.recover_on_drop = recover_on_drop;
        }
    }

    impl<'a, T, Target> ops::Drop for PoisonGuard<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        #[track_caller]
        fn drop(&mut self) {
            self.target.poisoned = if thread::panicking() {
                PoisonState::from_panic(None)
            } else if !self.recover_on_drop {
                PoisonState::from_err(None)
            } else {
                PoisonState::unpoisoned()
            };
        }
    }

    impl<'a, T, Target> fmt::Debug for PoisonGuard<'a, T, Target>
    where
        T: fmt::Debug,
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.debug_struct("PoisonGuard")
                .field(&"value", &self.target.value)
                .finish()
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
        /**
        Recover a poisoned value.

        The guard may or may not actually contain a previous value.
        If it doesn't, the caller will have to produce one to recover with.
        */
        #[track_caller]
        pub fn recover(mut self, f: impl FnOnce(&mut T)) -> PoisonGuard<'a, T, Target> {
            f(&mut self.target.value);
            self.target.poisoned = PoisonState::unpoisoned();

            PoisonGuard {
                target: self.target,
                recover_on_drop: true,
                _marker: Default::default(),
            }
        }

        /**
        Try recover a poisoned value.

        The guard may or may not actually contain a previous value.
        If it doesn't, the caller will have to produce one to recover with.
        */
        #[track_caller]
        pub fn try_recover<E>(
            mut self,
            f: impl FnOnce(&mut T) -> Result<(), E>,
        ) -> Result<PoisonGuard<'a, T, Target>, PoisonRecover<'a, T, Target>>
        where
            E: Error + Send + Sync + 'static,
        {
            match f(&mut self.target.value) {
                Ok(()) => {
                    self.target.poisoned = PoisonState::unpoisoned();

                    Ok(PoisonGuard {
                        target: self.target,
                        recover_on_drop: true,
                        _marker: Default::default(),
                    })
                }
                Err(e) => {
                    self.target.poisoned = PoisonState::from_err(Some(Box::new(e)));

                    Err(self)
                }
            }
        }
    }

    impl<'a, T, Target> fmt::Debug for PoisonRecover<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.debug_struct("PoisonRecover")
                .field(&"source", &self.target.poisoned)
                .finish()
        }
    }

    impl<'a, T, Target> fmt::Display for PoisonRecover<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            fmt::Display::fmt(&self.target.poisoned, f)
        }
    }

    impl<'a, T, Target> AsRef<dyn Error + Send + Sync + 'static> for PoisonRecover<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        fn as_ref(&self) -> &(dyn Error + Send + Sync + 'static) {
            &self.target.poisoned
        }
    }

    // TODO: Do we want to return a concrete error type here?
    impl<'a, T, Target> From<PoisonRecover<'a, T, Target>> for Box<dyn Error + 'static>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        fn from(guard: PoisonRecover<'a, T, Target>) -> Self {
            Box::new(guard.target.poisoned.clone())
        }
    }

    impl<'a, T, Target> From<PoisonRecover<'a, T, Target>> for Box<dyn Error + Send + 'static>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        fn from(guard: PoisonRecover<'a, T, Target>) -> Self {
            Box::new(guard.target.poisoned.clone())
        }
    }

    impl<'a, T, Target> From<PoisonRecover<'a, T, Target>> for Box<dyn Error + Send + Sync + 'static>
    where
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        fn from(guard: PoisonRecover<'a, T, Target>) -> Self {
            Box::new(guard.target.poisoned.clone())
        }
    }

    #[derive(Clone)]
    enum PoisonState {
        Panic(Arc<PoisonStatePanic>),
        Err(Arc<PoisonStateErr>),
        UnknownPanic(Arc<Backtrace>),
        UnknownErr(Arc<Backtrace>),
        Sentinel,
        Unpoisoned,
    }

    struct PoisonStatePanic {
        backtrace: Backtrace,
        payload: Cow<'static, str>,
    }

    struct PoisonStateErr {
        backtrace: Backtrace,
        source: Box<dyn Error + Send + Sync>,
    }

    impl PoisonState {
        #[track_caller]
        fn from_err(err: Option<Box<dyn Error + Send + Sync>>) -> Self {
            if let Some(err) = err {
                PoisonState::Err(Arc::new(PoisonStateErr {
                    backtrace: Backtrace::capture(),
                    source: err,
                }))
            } else {
                PoisonState::UnknownErr(Arc::new(Backtrace::capture()))
            }
        }

        #[track_caller]
        fn from_panic(panic: Option<Box<dyn Any + Send>>) -> Self {
            let panic = panic.and_then(|mut panic| {
                if let Some(msg) = panic.downcast_ref::<&'static str>() {
                    return Some(Cow::Borrowed(*msg));
                }

                if let Some(msg) = panic.downcast_mut::<String>() {
                    return Some(Cow::Owned(mem::take(&mut *msg)));
                }

                None
            });

            if let Some(panic) = panic {
                PoisonState::Panic(Arc::new(PoisonStatePanic {
                    backtrace: Backtrace::capture(),
                    payload: panic,
                }))
            } else {
                PoisonState::UnknownPanic(Arc::new(Backtrace::capture()))
            }
        }

        fn unpoisoned() -> Self {
            PoisonState::Unpoisoned
        }

        fn sentinel() -> Self {
            PoisonState::Sentinel
        }

        fn is_poisoned(&self) -> bool {
            !matches!(self, PoisonState::Unpoisoned)
        }
    }

    impl fmt::Debug for PoisonState {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match self {
                PoisonState::Panic(panic) => f
                    .debug_struct("PoisonState")
                    .field(&"panic", &panic.payload)
                    .field(&"backtrace", &panic.backtrace)
                    .finish(),
                PoisonState::UnknownPanic(backtrace) => f
                    .debug_struct("PoisonState")
                    .field(&"panic", &"<unknown>")
                    .field(&"backtrace", &backtrace)
                    .finish(),
                PoisonState::Err(err) => f
                    .debug_struct("PoisonState")
                    .field(&"err", &err.source)
                    .field(&"backtrace", &err.backtrace)
                    .finish(),
                PoisonState::UnknownErr(backtrace) => f
                    .debug_struct("PoisonState")
                    .field(&"err", &"<unknown>")
                    .field(&"backtrace", &backtrace)
                    .finish(),
                PoisonState::Sentinel => f.debug_struct("PoisonState").finish(),
                PoisonState::Unpoisoned => f.debug_struct("PoisonState").finish(),
            }
        }
    }

    impl fmt::Display for PoisonState {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match self {
                PoisonState::Panic(panic) => {
                    write!(f, "a guard was poisoned by a panic at '{}'", panic.payload)
                }
                PoisonState::UnknownPanic(_) => write!(f, "a guard was poisoned by a panic"),
                PoisonState::Err(_) => write!(f, "a guard was poisoned by an error"),
                PoisonState::UnknownErr(_) => write!(f, "a guard was poisoned by an error"),
                PoisonState::Sentinel => write!(f, "a guard was poisoned"),
                PoisonState::Unpoisoned => write!(f, "a guard was not poisoned"),
            }
        }
    }

    impl Error for PoisonState {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            if let PoisonState::Err(ref err) = self {
                Some(&*err.source)
            } else {
                None
            }
        }

        fn backtrace(&self) -> Option<&Backtrace> {
            match self {
                PoisonState::Err(ref err) => Some(&err.backtrace),
                PoisonState::Panic(ref panic) => Some(&panic.backtrace),
                PoisonState::UnknownErr(ref backtrace) => Some(&**backtrace),
                PoisonState::UnknownPanic(ref backtrace) => Some(&**backtrace),
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{guard::*, poison::*};
    use std::{error::Error, io, mem, ops, panic, ptr};

    #[test]
    fn poison_size() {
        assert!(mem::size_of::<Poison<()>>() <= (mem::size_of::<usize>() * 2));
    }

    #[test]
    fn unpoisoned_guard_can_access_value() {
        let mut v = Poison::new(42);

        let mut guard = v.poison().unwrap();

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
        std::mem::forget(v.poison().unwrap());
        assert!(v.is_poisoned());

        // Unpoison the guard and decrement the value back down
        let guard = v
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
        std::mem::forget(v.poison().unwrap());
        assert!(v.is_poisoned());

        // Unpoison the guard and decrement the value back down
        let guard = v
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
        let v = Poison::catch_unwind(|| panic!("lol"));

        assert!(v.is_poisoned());
    }

    #[test]
    fn guard_poisons_on_panic() {
        let mut v = Poison::new(42);

        let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            let mut guard = v.poison().unwrap();

            *guard += 1;

            panic!("lol");
        }));

        assert!(v.is_poisoned());
    }

    #[test]
    fn catch_unwind_poison() {
        let mut v = Poison::catch_unwind(|| panic!("lol"));
        let _ = v.poison().unwrap();
    }

    #[test]
    fn try_catch_unwind_err() {
        let mut v = Poison::try_catch_unwind(|| Err::<(), io::Error>(io::ErrorKind::Other.into()));
        let _ = v.poison().unwrap();
    }

    #[test]
    fn convert_poisoned_guard_into_error() {
        fn try_with(v: &mut Poison<i32>) -> Result<(), Box<dyn Error + 'static>> {
            let guard = v.poison()?;

            assert_eq!(42, *guard);

            Ok(())
        }

        assert!(try_with(&mut Poison::new(42)).is_ok());
        assert!(try_with(&mut Poison::catch_unwind(|| panic!("lol"))).is_err());
    }

    mod lazy {
        use super::*;

        use std::lazy::SyncLazy as Lazy;

        #[test]
        fn poisoning_lazy_ok() {
            static LAZY: Lazy<Poison<i32>> = Lazy::new(|| Poison::catch_unwind(|| 42));

            assert_eq!(42, *LAZY.get().unwrap());
        }

        #[test]
        fn poisoning_lazy_panic() {
            static LAZY: Lazy<Poison<i32>> = Lazy::new(|| Poison::catch_unwind(|| panic!("lol")));

            assert!(LAZY.is_poisoned());
        }
    }

    mod mutex {
        use super::*;

        use std::{sync::Arc, thread};

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
            PoisonGuard::recover_on_drop(&mut v, false);
            drop(v);

            let guard = mutex
                .lock()
                .poison()
                .unwrap_or_else(|guard| guard.recover(|v| *v = 42));

            assert_eq!(42, *guard);
        }

        #[test]
        fn propagating_across_threads() {
            let mutex = Arc::new(Mutex::new(Poison::new(42)));

            let t = {
                let mutex = mutex.clone();
                thread::spawn(move || {
                    let mut guard = mutex.lock().poison().unwrap();

                    *guard += 1;

                    panic!("lol");
                })
            };

            assert!(t.join().is_err());

            assert!(mutex.lock().is_poisoned());
        }
    }

    mod guard {
        use super::*;

        use std::{
            io,
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
                        *elem = mem::MaybeUninit::new(*i as u8);
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
                        *elem = mem::MaybeUninit::new(*i as u8);
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

            let p = Poison::catch_unwind(|| {
                let arr: [DropValue; 16] = init_unwind_safe(
                    0usize,
                    |i, mut uninit| {
                        for elem in uninit.array_mut() {
                            *elem = mem::MaybeUninit::new(DropValue(drop_count.clone()));
                            init_count += 1;

                            *i += 1;
                            if *i == 8 {
                                panic!("lol");
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

            let p = Poison::try_catch_unwind(|| {
                let arr: Result<[DropValue; 16], io::Error> = try_init_unwind_safe(
                    0usize,
                    |i, mut uninit| {
                        for elem in uninit.array_mut() {
                            *elem = mem::MaybeUninit::new(DropValue(drop_count.clone()));
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

            let p = Poison::catch_unwind(|| {
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
                            panic!("lol");
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
        fn init_guard_cheeky_mem_swap() {
            unimplemented!("what can we break with `mem::swap` (internally in `InitArgs`)?")
        }

        #[test]
        fn init_guard_try_panic_on_err() {
            unimplemented!("make sure we don't call `Drop` on the initialized value if `unwind` closure panics")
        }
    }
}
