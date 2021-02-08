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

    ## Examples

    Creating an unpoisoned value:

    ```
    # use poison_guard::Poison;
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
    # #![feature(once_cell)]
    # use poison_guard::Poison;
    # fn some_failure_condition() -> bool { false }
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
    use std::{io, lazy::SyncLazy as Lazy};

    # use poison_guard::Poison;
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
    # #![feature(once_cell)]
    # use poison_guard::Poison;
    # fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::lazy::SyncLazy as Lazy;

    static SHARED: Lazy<Poison<i32>> = Lazy::new(|| Poison::new(42));

    let value = SHARED.get()?;

    assert_eq!(42, *value);
    # Ok(())
    # }
    ```
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

    Scopes can be used to catch panics or errors that are encountered while using a value.

    ## Examples

    Creating a synchronous scope:

    ```
    # use std::io;
    # use poison_guard::Poison;
    # fn err_too_big() -> io::Error { io::ErrorKind::Other.into() }
    # fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut p = Poison::new(1);

    let mut scope = Poison::scope(p.as_mut().poison()?);

    scope.try_catch_unwind(|v| {
        *v += 1;

        if *v > 10 {
            Err(err_too_big())
        } else {
            Ok(())
        }
    })?;

    let mut guard = scope.poison()?;

    assert_eq!(2, *guard);
    # Ok(())
    # }
    ```

    Creating an asynchronous scope:

    ```
    # use std::io;
    # use poison_guard::Poison;
    # fn err_too_big() -> io::Error { io::ErrorKind::Other.into() }
    # async fn some_other_work(i: &mut i32) -> Result<(), io::Error> { Err(io::ErrorKind::Other.into()) }
    # fn main() {}
    # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut p = Poison::new(1);

    let mut scope = Poison::scope(p.as_mut().poison()?);

    scope.try_catch_unwind(|v| async move {
        *v += 1;

        some_other_work(v).await?;

        if *v > 10 {
            Err(err_too_big())
        } else {
            Ok(())
        }
    }).await?;

    let mut guard = scope.poison()?;

    assert_eq!(2, *guard);
    # Ok(())
    # }
    ```
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

    It's not usually necessary to poison a guard manually.
    */
    #[track_caller]
    pub fn err<'a, E, Target>(
        guard: PoisonGuard<'a, T, Target>,
        e: E,
    ) -> PoisonRecover<'a, T, Target>
    where
        E: Into<Box<dyn Error + Send + Sync>>,
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        let mut target = PoisonGuard::take(guard);
        target.state.then_to_err(Some(e.into()));

        PoisonRecover::new(target)
    }
}

impl<T> AsMut<Poison<T>> for Poison<T> {
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}
