/*!
Unwind-safe containers.
*/

use std::{
    error::Error,
    ops,
    panic::{self, Location},
};

mod error;
mod guard;
mod recover;
mod scope;

pub use self::{
    error::PoisonError,
    guard::PoisonGuard,
    recover::PoisonRecover,
    scope::{PoisonScope, TryCatchUnwind},
};

use self::error::PoisonState;

/**
A container that holds a potentially poisoned value.

`Poison<T>` doesn't mange its own synchronization, so it needs to be wrapped in a `Once` or a
`Mutex` so it can be shared.
*/
pub struct Poison<T> {
    value: T,
    state: PoisonState,
}

impl<T> Poison<T> {
    /**
    Create a new `Poison<T>` with a valid inner value.

    # Examples

    Creating an unpoisoned value:

    ```
    # use poison_guard::Poison;
    let mut value = Poison::new(42);

    // The value isn't poisoned, so we can access it
    let mut guard = value.poison().unwrap();

    assert_eq!(42, *guard);
    ```
    */
    pub fn new(v: T) -> Self {
        Poison {
            value: v,
            state: PoisonState::from_unpoisoned(),
        }
    }

    /**
    Try create a new `Poison<T>` with an initialization function that may unwind.

    If initialization does unwind then the panic payload will be caught and stashed inside the
    `Poison<T>`. Any attempt to access the poisoned value will instead return this payload unless
    the `Poison<T>` is restored.

    ## Examples

    Using `Poison<T>` in a lazy static:

    ```
    # #![feature(once_cell)]
    # use poison_guard::Poison;
    # fn some_failure_condition() -> bool { unimplemented!() }
    # fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::lazy::SyncLazy as Lazy;

    static SHARED: Lazy<Poison<String>> = Lazy::new(|| Poison::new_catch_unwind(|| {
        let mut value = String::from("Hello");

        if some_failure_condition() {
            panic!("couldn't make a value to store");
        } else {
            value.push_str(", world!");
        }

        value
    }));

    let value = SHARED.get()?;

    assert_eq!("Hello, world!", &*value);
    # Ok(())
    # }
    ```
    */
    #[track_caller]
    pub fn new_catch_unwind(f: impl FnOnce() -> T) -> Self
    where
        T: Default,
    {
        match panic::catch_unwind(panic::AssertUnwindSafe(f)) {
            Ok(v) => Poison {
                value: v,
                state: PoisonState::from_unpoisoned(),
            },
            Err(panic) => Poison {
                value: Default::default(),
                state: PoisonState::from_panic(Location::caller(), Some(panic)),
            },
        }
    }

    /**
    Try create a new `Poison<T>` with an initialization function that may fail or unwind.

    This is a fallible version of `new_catch_unwind`. If initialization does unwind then the error
    or panic payload will be caught and stashed inside the `Poison<T>`. Any attempt to access the
    poisoned value will instead return this payload unless the `Poison<T>` is restored.

    ## Examples

    Using `Poison<T>` in a lazy static:

    ```
    # #![feature(once_cell)]
    # use poison_guard::Poison;
    # fn check_some_things() -> Result<bool, io::Error> { unimplemented!() }
    # fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::{io, lazy::SyncLazy as Lazy};

    static SHARED: Lazy<Poison<String>> = Lazy::new(|| Poison::try_new_catch_unwind(|| {
        let mut value = String::from("Hello");

        check_some_things()?;

        Ok::<String, io::Error>(value)
    }));

    let value = SHARED.get()?;

    assert_eq!("Hello, world!", &*value);
    # Ok(())
    # }
    ```
    */
    #[track_caller]
    pub fn try_new_catch_unwind<E>(f: impl FnOnce() -> Result<T, E>) -> Self
    where
        T: Default,
        E: Error + Send + Sync + 'static,
    {
        match panic::catch_unwind(panic::AssertUnwindSafe(f)) {
            Ok(Ok(v)) => Poison {
                value: v,
                state: PoisonState::from_unpoisoned(),
            },
            Ok(Err(e)) => Poison {
                value: Default::default(),
                state: PoisonState::from_err(Location::caller(), Some(Box::new(e))),
            },
            Err(panic) => Poison {
                value: Default::default(),
                state: PoisonState::from_panic(Location::caller(), Some(panic)),
            },
        }
    }

    /**
    Whether or not the value is poisoned.
    */
    pub fn is_poisoned(&self) -> bool {
        self.state.is_poisoned()
    }

    /**
    Try get the inner value.

    This will return `Err` if the value is poisoned.
    */
    pub fn get<'a>(&'a self) -> Result<&'a T, PoisonRecover<'a, T, &'a Self>> {
        if self.is_poisoned() {
            Err(PoisonRecover::new(self))
        } else {
            Ok(&self.value)
        }
    }

    /**
    Try poison the value and return a guard to it.

    When the guard is dropped the value will be unpoisoned, unless a panic unwound through it.

    # Examples

    Poisoning a local variable or field using `as_mut`:

    ```
    # use poison_guard::poison::Poison;
    let mut v = Poison::new(42);

    let guard = v.as_mut().poison().unwrap();

    assert_eq!(42, *guard);
    ```

    Poisoning a mutex:

    ```
    # use poison_guard::poison::Poison;
    use parking_lot::Mutex;

    let mutex = Mutex::new(Poison::new(42));

    let guard = mutex.lock().poison().unwrap();

    assert_eq!(42, *guard);
    ```
    */
    #[track_caller]
    pub fn poison<'a, Target>(
        self: Target,
    ) -> Result<PoisonGuard<'a, T, Target>, PoisonRecover<'a, T, Target>>
    where
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        if self.is_poisoned() {
            Err(PoisonRecover::new(self))
        } else {
            Ok(PoisonGuard::new(self))
        }
    }

    /**
    Convert a guard into a scope.
    */
    #[track_caller]
    pub fn scope<'a, Target>(guard: PoisonGuard<'a, T, Target>) -> PoisonScope<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        let target = PoisonGuard::take(guard);

        PoisonScope::<T, Target>::new(target)
    }

    /**
    Poison a guard explicitly with an error.
    */
    #[track_caller]
    pub fn err<'a, E, Target>(
        guard: PoisonGuard<'a, T, Target>,
        e: E,
    ) -> PoisonRecover<'a, T, Target>
    where
        E: Error + Send + Sync + 'static,
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        let mut target = PoisonGuard::take(guard);
        target.state.then_to_err(Some(Box::new(e)));

        PoisonRecover::new(target)
    }
}

impl<T> AsMut<Poison<T>> for Poison<T> {
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}
