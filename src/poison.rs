/*!
Unwind-safe containers.
*/

use std::{
    any::Any,
    backtrace::Backtrace,
    borrow::Cow,
    error::Error,
    fmt, mem, ops,
    panic::{self, Location},
    ptr,
    sync::Arc,
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
    #[track_caller]
    pub fn new_catch_unwind(f: impl FnOnce() -> T) -> Self
    where
        T: Default,
    {
        match panic::catch_unwind(panic::AssertUnwindSafe(f)) {
            Ok(v) => Poison {
                value: v,
                poisoned: PoisonState::unpoisoned(),
            },
            Err(panic) => Poison {
                value: Default::default(),
                poisoned: PoisonState::from_panic(Location::caller(), Some(panic)),
            },
        }
    }

    /**
    Try create a new `Poison<T>` with an initialization function that may fail or unwind.

    If initialization does unwind then the error or panic payload will be caught and stashed inside the `Poison<T>`.
    Any attempt to access the poisoned value will instead return this payload unless the `Poison<T>` is restored.
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
                poisoned: PoisonState::unpoisoned(),
            },
            Ok(Err(e)) => Poison {
                value: Default::default(),
                poisoned: PoisonState::from_err(Location::caller(), Some(Box::new(e))),
            },
            Err(panic) => Poison {
                value: Default::default(),
                poisoned: PoisonState::from_panic(Location::caller(), Some(panic)),
            },
        }
    }

    /**
    Whether or not the value is poisoned.
    */
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.is_poisoned()
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
        mut self: Target,
    ) -> Result<PoisonGuard<'a, T, Target>, PoisonRecover<'a, T, Target>>
    where
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        if self.is_poisoned() {
            Err(PoisonRecover::new(self))
        } else {
            self.poisoned = PoisonState::sentinel();

            Ok(PoisonGuard::new(self))
        }
    }

    /**
    Use a guard within a closure that may fail or unwind.

    If the closure fails or unwinds then the value will be poisoned with the given error.

    # Examples

    ```
    # use std::io::Error;
    # use poison_guard::poison::Poison;
    # fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut v = Poison::new(42);

    Poison::try_with_catch_unwind(
        v.as_mut()
            .poison()
            .unwrap_or_else(|poisoned| poisoned.recover(|v| *v = 42)),
        |guard| {
            *guard += 1;

            Ok::<(), Error>(())
        },
    )?;
    # Ok(())
    # }
    ```
    */
    // TODO: Should we just remove this in favour of `{enter/exit}_with`?
    #[track_caller]
    pub fn try_with_catch_unwind<'a, U, E, Target>(
        mut guard: PoisonGuard<'a, T, Target>,
        f: impl FnOnce(&mut T) -> Result<U, E>,
    ) -> Result<U, PoisonRecover<'a, T, Target>>
    where
        E: Error + Send + Sync + 'static,
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        match panic::catch_unwind(panic::AssertUnwindSafe(|| f(&mut *guard))) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => {
                let mut target = PoisonGuard::take(guard);
                target.poisoned.with_err(Some(Box::new(e)));

                Err(PoisonRecover::new(target))
            }
            Err(panic) => {
                let mut target = PoisonGuard::take(guard);
                target.poisoned.with_panic(Some(panic));

                Err(PoisonRecover::new(target))
            }
        }
    }

    /**
    Enter a strict scope where a regular poison guard must be unpoisoned manually.

    This is an alternative to `try_with_catch_unwind` for operations that don't naturally fit
    into a closure, such as async functions.
    */
    pub fn enter<'a, Target>(guard: PoisonGuard<'a, T, Target>) -> PoisonGuardStrict<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        let target = PoisonGuard::take(guard);
        PoisonGuardStrict::new(target)
    }

    /**
    Exit a strict scope successfully.

    The returned guard will unpoison on drop as normal.
    */
    pub fn exit_ok<'a, Target>(guard: PoisonGuardStrict<'a, T, Target>) -> PoisonGuard<'a, T, Target>
    where
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        let target = PoisonGuardStrict::take(guard);

        PoisonGuard::new(target)
    }

    /**
    Exit a strict scope with an error.

    The value will remain poisoned and must be recovered as normal.
    */
    pub fn exit_err<'a, E, Target>(guard: PoisonGuardStrict<'a, T, Target>, e: E) -> PoisonRecover<'a, T, Target>
    where
        E: Error + Send + Sync + 'static,
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        let mut target = PoisonGuardStrict::take(guard);
        target.poisoned.with_err(Some(Box::new(e)));

        PoisonRecover::new(target)
    }
}

impl<T> AsMut<Poison<T>> for Poison<T> {
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}

/**
A guard for a valid value that will unpoison on drop.
*/
pub struct PoisonGuard<'a, T, Target = &'a mut Poison<T>>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    target: Target,
    _marker: std::marker::PhantomData<&'a mut T>,
}

impl<'a, T, Target> PoisonGuard<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    fn new(target: Target) -> PoisonGuard<'a, T, Target> {
        PoisonGuard {
            target,
            _marker: Default::default(),
        }
    }

    fn take(mut guard: Self) -> Target {
        let target = &mut guard.target as *mut Target;

        // Forgetting the struct itself here is ok, because the
        // other fields of `PoisonGuard` don't require `Drop`
        mem::forget(guard);

        // SAFETY: The target pointer is still valid
        unsafe { ptr::read(target) }
    }
}

impl<'a, T, Target> ops::Drop for PoisonGuard<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    #[track_caller]
    fn drop(&mut self) {
        if thread::panicking() {
            self.target.poisoned.with_panic(None);
        } else {
            self.target.poisoned = PoisonState::unpoisoned();
        }
    }
}

impl<'a, T, Target> fmt::Debug for PoisonGuard<'a, T, Target>
where
    T: fmt::Debug,
    Target: ops::DerefMut<Target = Poison<T>>,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("PoisonGuard")
            .field(&"value", &**self)
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
A guard for a valid value that must be unpoisoned manually.
*/
pub struct PoisonGuardStrict<'a, T, Target = &'a mut Poison<T>>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    target: Target,
    _marker: std::marker::PhantomData<&'a mut T>,
}

impl<'a, T, Target> PoisonGuardStrict<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    fn new(target: Target) -> PoisonGuardStrict<'a, T, Target> {
        PoisonGuardStrict {
            target,
            _marker: Default::default(),
        }
    }

    fn take(mut guard: Self) -> Target {
        let target = &mut guard.target as *mut Target;

        // Forgetting the struct itself here is ok, because the
        // other fields of `PoisonGuardStrict` don't require `Drop`
        mem::forget(guard);

        // SAFETY: The target pointer is still valid
        unsafe { ptr::read(target) }
    }
}

impl<'a, T, Target> ops::Drop for PoisonGuardStrict<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    #[track_caller]
    fn drop(&mut self) {
        if thread::panicking() {
            self.target.poisoned.with_panic(None);
        } else {
            self.target.poisoned.with_err(None);
        }
    }
}

impl<'a, T, Target> fmt::Debug for PoisonGuardStrict<'a, T, Target>
where
    T: fmt::Debug,
    Target: ops::DerefMut<Target = Poison<T>>,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("PoisonGuardStrict")
            .field(&"value", &**self)
            .finish()
    }
}

impl<'a, T, Target> ops::Deref for PoisonGuardStrict<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    type Target = T;

    fn deref(&self) -> &T {
        &self.target.value
    }
}

impl<'a, T, Target> ops::DerefMut for PoisonGuardStrict<'a, T, Target>
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
pub struct PoisonRecover<'a, T, Target = &'a mut Poison<T>> {
    target: Target,
    _marker: std::marker::PhantomData<&'a mut T>,
}

impl<'a, T, Target> PoisonRecover<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    /**
    Recover a poisoned value.
    */
    #[track_caller]
    pub fn recover(mut self, f: impl FnOnce(&mut T)) -> PoisonGuard<'a, T, Target> {
        f(&mut self.target.value);
        self.target.poisoned = PoisonState::unpoisoned();

        PoisonGuard::new(self.target)
    }

    /**
    Try recover a poisoned value.
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

                Ok(PoisonGuard::new(self.target))
            }
            Err(e) => {
                self.target.poisoned.with_err(Some(Box::new(e)));

                Err(self)
            }
        }
    }
}

impl<'a, T, Target> PoisonRecover<'a, T, Target>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn new(target: Target) -> PoisonRecover<'a, T, Target> {
        PoisonRecover {
            target,
            _marker: Default::default(),
        }
    }
}

impl<'a, T, Target> fmt::Debug for PoisonRecover<'a, T, Target>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("PoisonRecover")
            .field(&"source", &self.target.poisoned)
            .finish()
    }
}

impl<'a, T, Target> fmt::Display for PoisonRecover<'a, T, Target>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.target.poisoned, f)
    }
}

impl<'a, T, Target> AsRef<dyn Error + Send + Sync + 'static> for PoisonRecover<'a, T, Target>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn as_ref(&self) -> &(dyn Error + Send + Sync + 'static) {
        &self.target.poisoned
    }
}

impl<'a, T, Target> From<PoisonRecover<'a, T, Target>> for Box<dyn Error + 'static>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn from(guard: PoisonRecover<'a, T, Target>) -> Self {
        Box::new(guard.target.poisoned.clone())
    }
}

impl<'a, T, Target> From<PoisonRecover<'a, T, Target>> for Box<dyn Error + Send + 'static>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn from(guard: PoisonRecover<'a, T, Target>) -> Self {
        Box::new(guard.target.poisoned.clone())
    }
}

impl<'a, T, Target> From<PoisonRecover<'a, T, Target>> for Box<dyn Error + Send + Sync + 'static>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn from(guard: PoisonRecover<'a, T, Target>) -> Self {
        Box::new(guard.target.poisoned.clone())
    }
}

#[derive(Clone)]
enum PoisonState {
    CapturedPanic(Arc<CapturedPanic>),
    UnknownPanic(Arc<UnknownPanic>),
    CapturedErr(Arc<CapturedErr>),
    UnknownErr(Arc<UnknownErr>),
    Sentinel(&'static Location<'static>),
    Unpoisoned,
}

struct CapturedPanic {
    backtrace: Backtrace,
    location: &'static Location<'static>,
    payload: Cow<'static, str>,
}

struct UnknownPanic {
    backtrace: Backtrace,
    location: &'static Location<'static>,
}

struct CapturedErr {
    backtrace: Backtrace,
    location: &'static Location<'static>,
    source: Box<dyn Error + Send + Sync>,
}

struct UnknownErr {
    backtrace: Backtrace,
    location: &'static Location<'static>,
}

impl PoisonState {
    fn from_err(
        location: &'static Location<'static>,
        err: Option<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        if let Some(err) = err {
            PoisonState::CapturedErr(Arc::new(CapturedErr {
                backtrace: Backtrace::capture(),
                location,
                source: err,
            }))
        } else {
            PoisonState::UnknownErr(Arc::new(UnknownErr {
                backtrace: Backtrace::capture(),
                location,
            }))
        }
    }

    #[track_caller]
    fn with_err(&mut self, err: Option<Box<dyn Error + Send + Sync>>) {
        let location = if let PoisonState::Sentinel(location) = self {
            *location
        } else {
            Location::caller()
        };

        *self = PoisonState::from_err(location, err);
    }

    fn from_panic(
        location: &'static Location<'static>,
        panic: Option<Box<dyn Any + Send>>,
    ) -> Self {
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
            PoisonState::CapturedPanic(Arc::new(CapturedPanic {
                backtrace: Backtrace::capture(),
                location,
                payload: panic,
            }))
        } else {
            PoisonState::UnknownPanic(Arc::new(UnknownPanic {
                backtrace: Backtrace::capture(),
                location,
            }))
        }
    }

    #[track_caller]
    fn with_panic(&mut self, panic: Option<Box<dyn Any + Send>>) {
        let location = if let PoisonState::Sentinel(location) = self {
            *location
        } else {
            Location::caller()
        };

        *self = PoisonState::from_panic(location, panic);
    }

    fn unpoisoned() -> Self {
        PoisonState::Unpoisoned
    }

    #[track_caller]
    fn sentinel() -> Self {
        PoisonState::Sentinel(Location::caller())
    }

    fn is_poisoned(&self) -> bool {
        !matches!(self, PoisonState::Unpoisoned)
    }
}

impl fmt::Debug for PoisonState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PoisonState::CapturedPanic(panic) => f
                .debug_struct("PoisonState")
                .field(&"panic", &panic.payload)
                .field(&"location", &panic.location)
                .finish(),
            PoisonState::UnknownPanic(panic) => f
                .debug_struct("PoisonState")
                .field(&"panic", &"<unknown>")
                .field(&"location", &panic.location)
                .finish(),
            PoisonState::CapturedErr(err) => f
                .debug_struct("PoisonState")
                .field(&"err", &err.source)
                .field(&"location", &err.location)
                .finish(),
            PoisonState::UnknownErr(err) => f
                .debug_struct("PoisonState")
                .field(&"err", &"<unknown>")
                .field(&"location", &err.location)
                .finish(),
            PoisonState::Sentinel(location) => f
                .debug_struct("PoisonState")
                .field(&"location", &location)
                .finish(),
            PoisonState::Unpoisoned => f.debug_struct("PoisonState").finish(),
        }
    }
}

impl fmt::Display for PoisonState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PoisonState::CapturedPanic(panic) => {
                write!(
                    f,
                    "poisoned by a panic '{}' (the poisoning guard was acquired at '{}')",
                    panic.payload, panic.location
                )
            }
            PoisonState::UnknownPanic(panic) => write!(
                f,
                "poisoned by a panic (the poisoning guard was acquired at '{}')",
                panic.location
            ),
            PoisonState::CapturedErr(err) => write!(
                f,
                "poisoned by an error (the poisoning guard was acquired at '{}')",
                err.location
            ),
            PoisonState::UnknownErr(err) => write!(
                f,
                "poisoned by an error (the poisoning guard was acquired at '{}')",
                err.location
            ),
            PoisonState::Sentinel(location) => write!(
                f,
                "poisoned (the poisoning guard was acquired at '{}')",
                location
            ),
            PoisonState::Unpoisoned => write!(f, "a guard was not poisoned"),
        }
    }
}

impl Error for PoisonState {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        if let PoisonState::CapturedErr(ref err) = self {
            Some(&*err.source)
        } else {
            None
        }
    }

    fn backtrace(&self) -> Option<&Backtrace> {
        match self {
            PoisonState::CapturedErr(ref err) => Some(&err.backtrace),
            PoisonState::CapturedPanic(ref panic) => Some(&panic.backtrace),
            PoisonState::UnknownErr(ref err) => Some(&err.backtrace),
            PoisonState::UnknownPanic(ref panic) => Some(&panic.backtrace),
            _ => None,
        }
    }
}
