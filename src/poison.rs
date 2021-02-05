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
    #[track_caller]
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
    Whether or not the value is poisoned.
    */
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.is_poisoned()
    }

    /**
    Try get the inner value.

    This will return `Err` if the value is poisoned.
    */
    pub fn get(&self) -> Result<&T, &(dyn Error + Send + Sync + 'static)> {
        if self.is_poisoned() {
            Err(&self.poisoned)
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
    pub fn poison<'a, Target>(
        mut self: Target,
    ) -> Result<PoisonGuard<'a, T, Target>, PoisonRecover<'a, T, Target>>
    where
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        if self.is_poisoned() {
            Err(PoisonRecover {
                target: self,
                _marker: Default::default(),
            })
        } else {
            self.poisoned = PoisonState::sentinel();

            Ok(PoisonGuard {
                target: self,
                _marker: Default::default(),
            })
        }
    }

    /**
    Use a guard within a closure that may fail or unwind.

    If the closure fails then the value will be poisoned with the given error.

    # Examples

    ```
    # use std::io::Error;
    # use poison_guard::poison::Poison;
    # fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut v = Poison::new(42);

    Poison::try_with(
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
    #[track_caller]
    pub fn try_with<'a, U, E, Target>(
        mut guard: PoisonGuard<'a, T, Target>,
        f: impl FnOnce(&mut T) -> Result<U, E>,
    ) -> Result<U, PoisonRecover<'a, T, Target>>
    where
        E: Error + Send + Sync + 'static,
        Target: ops::DerefMut<Target = Poison<T>> + 'a,
    {
        match f(&mut *guard) {
            Ok(v) => Ok(v),
            Err(e) => {
                let mut target = PoisonGuard::take(guard);
                target.poisoned = PoisonState::from_err(Some(Box::new(e)));

                Err(PoisonRecover {
                    target,
                    _marker: Default::default(),
                })
            }
        }
    }
}

impl<T> AsMut<Poison<T>> for Poison<T> {
    fn as_mut(&mut self) -> &mut Self {
        self
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
    _marker: std::marker::PhantomData<&'a mut T>,
}

impl<'a, T, Target> PoisonGuard<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    pub fn by_ref<'b>(guard: &'b mut Self) -> PoisonGuard<'b, T> {
        PoisonGuard {
            target: &mut *guard.target,
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
        self.target.poisoned = if thread::panicking() {
            PoisonState::from_panic(None)
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
    */
    #[track_caller]
    pub fn recover(mut self, f: impl FnOnce(&mut T)) -> PoisonGuard<'a, T, Target> {
        f(&mut self.target.value);
        self.target.poisoned = PoisonState::unpoisoned();

        PoisonGuard {
            target: self.target,
            _marker: Default::default(),
        }
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

                Ok(PoisonGuard {
                    target: self.target,
                    _marker: Default::default(),
                })
            }
            Err(e) => {
                self.target.poisoned = PoisonState::from_err(Some(Box::new(e)));

                Err(self)
            }
        }
    }

    pub fn by_ref<'b>(guard: &'b mut Self) -> PoisonRecover<'b, T> {
        PoisonRecover {
            target: &mut *guard.target,
            _marker: Default::default(),
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
    CapturedPanic(Arc<CapturedPanic>),
    UnknownPanic(Arc<UnknownPanic>),
    CapturedErr(Arc<CapturedErr>),
    UnknownErr(Arc<UnknownErr>),
    Sentinel,
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
    #[track_caller]
    fn from_err(err: Option<Box<dyn Error + Send + Sync>>) -> Self {
        if let Some(err) = err {
            PoisonState::CapturedErr(Arc::new(CapturedErr {
                backtrace: Backtrace::capture(),
                location: Location::caller(),
                source: err,
            }))
        } else {
            PoisonState::UnknownErr(Arc::new(UnknownErr {
                backtrace: Backtrace::capture(),
                location: Location::caller(),
            }))
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
            PoisonState::CapturedPanic(Arc::new(CapturedPanic {
                backtrace: Backtrace::capture(),
                location: Location::caller(),
                payload: panic,
            }))
        } else {
            PoisonState::UnknownPanic(Arc::new(UnknownPanic {
                backtrace: Backtrace::capture(),
                location: Location::caller(),
            }))
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
            PoisonState::CapturedPanic(panic) => f
                .debug_struct("PoisonState")
                .field(&"panic", &panic.payload)
                .field(&"location", &panic.location)
                .field(&"backtrace", &panic.backtrace)
                .finish(),
            PoisonState::UnknownPanic(panic) => f
                .debug_struct("PoisonState")
                .field(&"panic", &"<unknown>")
                .field(&"location", &panic.location)
                .field(&"backtrace", &panic.backtrace)
                .finish(),
            PoisonState::CapturedErr(err) => f
                .debug_struct("PoisonState")
                .field(&"err", &err.source)
                .field(&"location", &err.location)
                .field(&"backtrace", &err.backtrace)
                .finish(),
            PoisonState::UnknownErr(err) => f
                .debug_struct("PoisonState")
                .field(&"err", &"<unknown>")
                .field(&"location", &err.location)
                .field(&"backtrace", &err.backtrace)
                .finish(),
            PoisonState::Sentinel => f.debug_struct("PoisonState").finish(),
            PoisonState::Unpoisoned => f.debug_struct("PoisonState").finish(),
        }
    }
}

impl fmt::Display for PoisonState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PoisonState::CapturedPanic(panic) => {
                write!(f, "a guard was poisoned by a panic at '{}'", panic.payload)
            }
            PoisonState::UnknownPanic(_) => write!(f, "a guard was poisoned by a panic"),
            PoisonState::CapturedErr(_) => write!(f, "a guard was poisoned by an error"),
            PoisonState::UnknownErr(_) => write!(f, "a guard was poisoned by an error"),
            PoisonState::Sentinel => write!(f, "a guard was poisoned"),
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
