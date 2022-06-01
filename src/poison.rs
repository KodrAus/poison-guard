/*!
Unwind-safe containers.
*/

use std::{
    error::Error,
    ops,
    panic::{
        self,
        Location,
        RefUnwindSafe,
    },
};

mod error;
mod guard;
mod recover;

pub use self::{
    error::PoisonError,
    guard::PoisonGuard,
    recover::PoisonRecover,
};

use self::error::PoisonState;

/**
A container that holds a potentially poisoned value.

`Poison<T>` doesn't manage its own synchronization, so it needs to be wrapped in a `Once` or a
`Mutex` so it can be shared.
*/
pub struct Poison<T> {
    value: T,
    state: PoisonState,
}

impl<T> RefUnwindSafe for Poison<T> {}

impl<T> Poison<T> {
    /**
    Create a new `Poison<T>` with a valid inner value.

    ## Examples

    Creating an unpoisoned value:

    ```
    use poison_guard::Poison;

    let mut value = Poison::new(42);

    // The value isn't poisoned, so we can access it
    let mut guard = value.as_mut().poison().unwrap();

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
    use once_cell::sync::Lazy;
    use poison_guard::Poison;

    # fn some_failure_condition() -> bool { false }
    # fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    use once_cell::sync::Lazy;
    use std::io;

    use poison_guard::Poison;

    # fn check_some_things(s: &mut String) -> Result<bool, io::Error> { Ok(false) }
    # fn main() -> Result<(), Box<dyn std::error::Error>> {
    static SHARED: Lazy<Poison<String>> = Lazy::new(|| Poison::try_new_catch_unwind(|| {
        let mut value = String::from("Hello");

        if check_some_things(&mut value)? {
            panic!("failed to check some things")
        } {
            value.push_str(", world!");
        }

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
        E: Into<Box<dyn Error + Send + Sync>>,
    {
        match panic::catch_unwind(panic::AssertUnwindSafe(f)) {
            Ok(Ok(v)) => Poison {
                value: v,
                state: PoisonState::from_unpoisoned(),
            },
            Ok(Err(e)) => Poison {
                value: Default::default(),
                state: PoisonState::from_err(Location::caller(), Some(e.into())),
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

    ## Examples

    Get the value of a potentially poisoned global value:

    ```
    use once_cell::sync::Lazy;
    use poison_guard::Poison;

    # fn main() -> Result<(), Box<dyn std::error::Error>> {
    static SHARED: Lazy<Poison<i32>> = Lazy::new(|| Poison::new(42));

    let value = SHARED.get()?;

    assert_eq!(42, *value);
    # Ok(())
    # }
    ```
    */
    pub fn get(&self) -> Result<&T, PoisonRecover<T, &Self>> {
        if self.is_poisoned() {
            Err(PoisonRecover::recover_to_poison_on_unwind(self))
        } else {
            Ok(&self.value)
        }
    }

    /**
    Get a guard to the value that will only poison if a panic unwinds through the guard.

    When the guard is dropped the value will be unpoisoned, unless a panic unwound through it.

    ## Examples

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

    let guard = Poison::on_unwind(mutex.lock()).unwrap();

    assert_eq!(42, *guard);
    ```
    */
    #[track_caller]
    pub fn on_unwind<'a, Target>(
        poison: Target,
    ) -> Result<PoisonGuard<'a, T, Target>, PoisonRecover<'a, T, Target>>
    where
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        if poison.is_poisoned() {
            Err(PoisonRecover::recover_to_poison_on_unwind(poison))
        } else {
            Ok(PoisonGuard::poison_on_unwind(poison))
        }
    }

    /**
    Get a guard to the value that will immediately poison and only unpoison with `Poison::recover` or `Poison::try_recover`.
    */
    #[track_caller]
    pub fn unless_recovered<'a, Target>(
        poison: Target,
    ) -> Result<PoisonGuard<'a, T, Target>, PoisonRecover<'a, T, Target>>
    where
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        if poison.is_poisoned() {
            Err(PoisonRecover::recover_to_poison_now(poison))
        } else {
            Ok(PoisonGuard::poison_now(poison))
        }
    }

    /**
    Poison a guard explicitly with an error.
    It's not usually necessary to poison a guard manually.
    */
    #[track_caller]
    pub fn err<'a, E, Target>(guard: PoisonGuard<'a, T, Target>, err: E)
    where
        E: Into<Box<dyn Error + Send + Sync>>,
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        PoisonGuard::poison_with_error(guard, err);
    }

    /**
    Recover a guard, unpoisoning it if it was poisoned.

    This method must be used to recover guards acquired through [`Poison::unless_recovered`].
    */
    pub fn recover<'a, Target>(guard: PoisonGuard<'a, T, Target>)
    where
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        PoisonGuard::unpoison_now(guard);
    }

    /**
    Try recover a guard based on a result.
    */
    pub fn try_recover<Target, O, E>(
        r: Result<O, E>,
        guard: PoisonGuard<T, Target>,
    ) -> Result<O, PoisonError>
    where
        E: Into<Box<dyn Error + Send + Sync>>,
        Target: ops::DerefMut<Target = Poison<T>>,
    {
        match r {
            Ok(ok) => {
                PoisonGuard::unpoison_now(guard);
                Ok(ok)
            }
            Err(err) => Err(PoisonGuard::poison_with_error(guard, err)),
        }
    }
}
