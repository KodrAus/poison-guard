use std::{
    any::Any,
    borrow::Cow,
    error::Error,
    fmt,
    mem,
    panic::Location,
    sync::Arc,
};

/**
An error indicating that a value was poisoned.
*/
#[derive(Clone)]
pub struct PoisonError(PoisonStateInner);

impl fmt::Debug for PoisonError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for PoisonError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Error for PoisonError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Error::source(&self.0)
    }
}

#[derive(Clone)]
pub(super) struct PoisonState(PoisonStateInner);

#[derive(Clone)]
enum PoisonStateInner {
    CapturedPanic(Arc<CapturedPanic>),
    UnknownPanic(Arc<UnknownPanic>),
    CapturedErr(Arc<CapturedErr>),
    UnknownErr(Arc<UnknownErr>),
    Guarded(&'static Location<'static>),
    Unpoisoned,
}

struct CapturedPanic {
    location: &'static Location<'static>,
    payload: Cow<'static, str>,
}

struct UnknownPanic {
    location: &'static Location<'static>,
}

struct CapturedErr {
    location: &'static Location<'static>,
    source: Box<dyn Error + Send + Sync>,
}

struct UnknownErr {
    location: &'static Location<'static>,
}

impl PoisonState {
    pub(super) fn from_unpoisoned() -> Self {
        PoisonState(PoisonStateInner::Unpoisoned)
    }

    pub(super) fn from_err(
        location: &'static Location<'static>,
        err: Option<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        PoisonState(if let Some(err) = err {
            PoisonStateInner::CapturedErr(Arc::new(CapturedErr {
                location,
                source: err,
            }))
        } else {
            PoisonStateInner::UnknownErr(Arc::new(UnknownErr { location }))
        })
    }

    pub(super) fn from_panic(
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

        PoisonState(if let Some(panic) = panic {
            PoisonStateInner::CapturedPanic(Arc::new(CapturedPanic {
                location,
                payload: panic,
            }))
        } else {
            PoisonStateInner::UnknownPanic(Arc::new(UnknownPanic { location }))
        })
    }

    #[track_caller]
    pub(super) fn guarded(&mut self) {
        *self = PoisonState(PoisonStateInner::Guarded(Location::caller()))
    }

    #[track_caller]
    pub(super) fn poison_with_error(&mut self, err: Option<Box<dyn Error + Send + Sync>>) {
        let location = if let PoisonStateInner::Guarded(location) = self.0 {
            location
        } else {
            Location::caller()
        };

        *self = PoisonState::from_err(location, err);
    }

    #[track_caller]
    pub(super) fn poison_with_panic(&mut self, panic: Option<Box<dyn Any + Send>>) {
        let location = if let PoisonStateInner::Guarded(location) = self.0 {
            location
        } else {
            Location::caller()
        };

        *self = PoisonState::from_panic(location, panic);
    }

    #[track_caller]
    pub(super) fn unpoison_if_guarded(&mut self) {
        if let PoisonStateInner::Guarded(_) = self.0 {
            *self = PoisonState::from_unpoisoned();
        }
    }

    #[track_caller]
    pub(super) fn unpoison(&mut self) {
        *self = PoisonState::from_unpoisoned();
    }

    pub(super) fn is_unpoisoned(&self) -> bool {
        matches!(self.0, PoisonStateInner::Unpoisoned)
    }

    pub(super) fn is_poisoned(&self) -> bool {
        !self.is_unpoisoned()
    }

    pub(super) fn to_error(&self) -> PoisonError {
        PoisonError(self.0.clone())
    }

    pub(super) fn as_dyn_error(&self) -> &(dyn Error + Send + Sync + 'static) {
        &self.0
    }

    pub(super) fn to_dyn_error(&self) -> Box<dyn Error + Send + Sync> {
        Box::new(self.0.clone())
    }
}

impl fmt::Debug for PoisonStateInner {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PoisonStateInner::CapturedPanic(panic) => f
                .debug_struct("PoisonState")
                .field(&"panic", &panic.payload)
                .field(&"location", &panic.location)
                .finish(),
            PoisonStateInner::UnknownPanic(panic) => f
                .debug_struct("PoisonState")
                .field(&"panic", &"<unknown>")
                .field(&"location", &panic.location)
                .finish(),
            PoisonStateInner::CapturedErr(err) => f
                .debug_struct("PoisonState")
                .field(&"err", &err.source)
                .field(&"location", &err.location)
                .finish(),
            PoisonStateInner::UnknownErr(err) => f
                .debug_struct("PoisonState")
                .field(&"err", &"<unknown>")
                .field(&"location", &err.location)
                .finish(),
            PoisonStateInner::Guarded(location) => f
                .debug_struct("PoisonState")
                .field(&"location", &location)
                .finish(),
            PoisonStateInner::Unpoisoned => f.debug_struct("PoisonState").finish(),
        }
    }
}

impl fmt::Display for PoisonStateInner {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PoisonStateInner::CapturedPanic(panic) => {
                write!(
                    f,
                    "poisoned by a panic '{}' (the poisoning guard was acquired at '{}')",
                    panic.payload, panic.location
                )
            }
            PoisonStateInner::UnknownPanic(panic) => write!(
                f,
                "poisoned by a panic (the poisoning guard was acquired at '{}')",
                panic.location
            ),
            PoisonStateInner::CapturedErr(err) => write!(
                f,
                "poisoned by an error (the poisoning guard was acquired at '{}')",
                err.location
            ),
            PoisonStateInner::UnknownErr(err) => write!(
                f,
                "poisoned by an error (the poisoning guard was acquired at '{}')",
                err.location
            ),
            PoisonStateInner::Guarded(location) => write!(
                f,
                "poisoned (the poisoning guard was acquired at '{}')",
                location
            ),
            PoisonStateInner::Unpoisoned => write!(f, "a guard was not poisoned"),
        }
    }
}

impl Error for PoisonStateInner {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        if let PoisonStateInner::CapturedErr(ref err) = self {
            Some(&*err.source)
        } else {
            None
        }
    }
}
