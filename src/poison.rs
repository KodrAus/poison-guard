/*!
Unwind-safe containers.
*/

use std::{
    error::Error,
    ops,
    panic::{self, Location, RefUnwindSafe},
};

mod error;
mod guard;
mod recover;

pub use self::{error::PoisonError, guard::PoisonGuard, recover::PoisonRecover};

use self::error::PoisonState;

/**
A container that holds a potentially poisoned value.

`Poison<T>` doesn't manage its own synchronization, so it needs to be wrapped in a `Once` or a
`Mutex` so it can be shared.

## Protecting state

Wrapping some state in a `Poison<T>` requires specific guard types to access. These guards
will _poison_ the state if they observe panics or other early returns:

```should_panic
use poison_guard::Poison;
use parking_lot::Mutex;
use std::sync::Arc;

// Arc<Mutex<Poison<T>> is a...
// ...reference counted shared value...
// ...protected by a lock...
// ... that will poison on panics.
let shared_state = Arc::new(Mutex::new(Poison::new(Vec::new())));

// Use Poison::on_unwind to get a guard that will protect against panics
let mut guard = Poison::on_unwind(shared_state.lock()).unwrap();

// If this call panics the guard will remain poisoned
guard[2] = 42;

// Once the guard goes out of scope the value will unlock and unpoison
```

See [`Poison::new`] for examples on creating `Poison<T>`s.

There are two methods for acquiring guards to access state protected by a `Poison<T>`.
They're both static rather than inherent:

- [`Poison::on_unwind`] for guards that only poison if a panic unwinds through them.
- [`Poison::unless_recovered`] for guards that remain poisoned unless they're explicitly recovered
after operating on their state. These also protect against early returns from `?`.

## Recovering state

If state protected by a `Poison<T>` becomes poisoned then it can be recovered:

```
use poison_guard::Poison;
use parking_lot::Mutex;
use std::sync::Arc;

fn with_state(state: Arc<Mutex<Poison<Vec<i32>>>>) {
    // If a previous caller poisoned the value we'll need to recover it
    let mut guard = match Poison::on_unwind(state.lock()) {
        Ok(guard) => guard,
        Err(recover) => recover.recover_with(|poisoned| {
            // There's something wrong with this Vec...
            // Let's just clear it and call it unpoisoned
            poisoned.clear();
        })
    };

    // Now we can use the state as normal
    guard.push(42);
}
```

See [`PoisonRecover`] for the methods available to recover poisoned values.

## Propagating failures

Not all state can be recovered after it's been poisoned. In these cases, the original failure
that caused the value to be poisoned can be propagated through `.unwrap()` or `?` when attempting
to acquire a guard:

```
use poison_guard::Poison;
use parking_lot::Mutex;
use std::sync::Arc;

fn with_state(state: Arc<Mutex<Poison<Vec<i32>>>>) {
    // If a previous caller poisoned the value we'll propagate the error
    // The `Poison<T>` is a safety net that surfaces bugs that broke the state
    // to begin with, rather than trying to restore it and carry on
    let mut guard = Poison::on_unwind(state.lock()).unwrap();

    guard.push(42);
}
```
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

    let mut value = Poison::new(41);

    // The value isn't poisoned, so we can access it
    let mut guard = Poison::on_unwind(&mut value).unwrap();

    *guard += 1;

    assert_eq!(42, *guard);
    ```

    See also [`Poison::new_catch_unwind`] and [`Poison::try_new_catch_unwind`] for other
    ways to make a `Poison<T>` from a fallible constructor.
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
    use poison_guard::Poison;
    use std::io;

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

    If this method returns `true` then [`Poison::get`], [`Poison::on_unwind`], [`Poison::unless_recovered`]
    etc will return `Ok`. Otherwise these methods will return `Err` with a recovery guard.
    */
    pub fn is_poisoned(&self) -> bool {
        self.state.is_poisoned()
    }

    /**
    Try get the inner value.

    This will return `Err` if the value is poisoned. The recovery guard returned in the poisoned
    case can be converted into a standard error type.

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
    If the guard is forgotten through `mem::forget` then the value will also be poisoned.

    See [`Poison::unless_recovered`] for an alternative to this method that also poisons on
    other early returns, like `?`.

    ## Examples

    Guarding a local variable or field:

    ```
    use poison_guard::Poison;

    let mut v = Poison::new(42);

    let guard = Poison::on_unwind(&mut v).unwrap();

    assert_eq!(42, *guard);
    ```

    Guarding a mutex:

    ```
    use poison_guard::Poison;
    use parking_lot::Mutex;

    // This type is semantically equivalent to the standard library's `Mutex`.
    // `parking_lot` doesn't implement poisoning itself, it will simply unlock
    // if a panic unwinds through a guard.
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
    Get a guard to the value that will immediately poison and only unpoison with [`Poison::recover`] or [`Poison::try_recover`].

    This method is an alternative to [`Poison::on_unwind`] that can also protect state against early
    returns through non-panicking control flow, like `?`.

    ## Examples

    Guarding a local variable or field:

    ```
    # fn some_fallible_operation(_: &mut i32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> { Ok(()) }
    # fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use poison_guard::Poison;

    let mut v = Poison::new(42);

    let mut guard = Poison::unless_recovered(&mut v)?;

    // If this call fails the value will remain poisoned
    some_fallible_operation(&mut guard)?;

    // If we get this far then the state is still valid, so return the guard and unpoison
    Poison::recover(guard);
    # Ok(())
    # }
    ```

    Poisoning a mutex:

    ```
    # fn some_fallible_operation(_: &mut i32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> { Ok(()) }
    # fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use poison_guard::Poison;
    use parking_lot::Mutex;

    let mutex = Mutex::new(Poison::new(42));

    let mut guard = Poison::unless_recovered(mutex.lock())?;

    // If this call fails the value will remain poisoned
    some_fallible_operation(&mut guard)?;

    // If we get this far then the state is still valid, so return the guard and unpoison
    Poison::recover(guard);
    # Ok(())
    # }
    ```
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
    Recover a guard, unpoisoning it if it was poisoned.

    This method must be used to recover guards acquired through [`Poison::unless_recovered`].
    Guards acquired through [`Poison::on_unwind`] can also be returned this way, but it isn't
    necessary to do so.

    # Examples

    Guarding a local variable or field:

    ```
    # fn some_fallible_operation(_: &mut i32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> { Ok(()) }
    # fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use poison_guard::Poison;

    let mut v = Poison::new(42);

    let mut guard = Poison::unless_recovered(&mut v)?;

    // If this call fails the value will remain poisoned
    some_fallible_operation(&mut guard)?;

    // If we get this far then the state is still valid, so return the guard and unpoison
    Poison::recover(guard);
    # Ok(())
    # }
    ```
    */
    pub fn recover<'a, Target>(guard: PoisonGuard<'a, T, Target>)
    where
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        PoisonGuard::unpoison_now(guard);
    }

    /**
    Try recover a guard based on a result.

    This method can be used to wrap a call that operates on the underlying value. If the operation
    fails then the value will be poisoned. This approach differs from [`Poison::recover`] by
    capturing the error and storing it in the `Poison<T>`. Future attempts to access the value will
    include the original error.

    ## Examples

    Guarding a local variable or field:

    ```
    # fn some_fallible_operation(_: &mut i32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> { Ok(()) }
    # fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use poison_guard::Poison;

    let mut v = Poison::new(42);

    let mut guard = Poison::unless_recovered(&mut v)?;

    // If this call fails the value will remain poisoned
    // The error will be captured and a wrapper will be returned
    Poison::try_recover(some_fallible_operation(&mut guard), guard)?;
    # Ok(())
    # }
    ```
    */
    #[track_caller]
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
