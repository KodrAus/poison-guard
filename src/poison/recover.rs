use std::{error::Error, fmt, marker, ops};

use super::{Poison, PoisonError, PoisonGuard};

/**
A guard for a poisoned value.
*/
pub struct PoisonRecover<'a, T, Target = &'a mut Poison<T>> {
    target: Target,
    _marker: marker::PhantomData<&'a mut T>,
}

impl<'a, T, Target> PoisonRecover<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    /**
    Recover a poisoned value.

    This method won't make any changes to the underlying value.
    After this call, any future accesses to the value will succeed.
    */
    #[track_caller]
    pub fn recover(self) -> PoisonGuard<'a, T, Target> {
        PoisonGuard::new(self.target)
    }

    /**
    Recover a poisoned value with the given closure.

    After this call, any future accesses to the value will succeed.
    */
    #[track_caller]
    pub fn recover_with(mut self, f: impl FnOnce(&mut T)) -> PoisonGuard<'a, T, Target> {
        f(&mut self.target.value);

        PoisonGuard::new(self.target)
    }

    /**
    Try recover a poisoned value with the given closure.

    If this call succeeds, any future accesses to the value will succeed.
    */
    #[track_caller]
    pub fn try_recover_with<E>(
        mut self,
        f: impl FnOnce(&mut T) -> Result<(), E>,
    ) -> Result<PoisonGuard<'a, T, Target>, PoisonRecover<'a, T, Target>>
    where
        E: Into<Box<dyn Error + Send + Sync>>,
    {
        match f(&mut self.target.value) {
            Ok(()) => {
                self.target.state.then_to_unpoisoned();

                Ok(PoisonGuard::new(self.target))
            }
            Err(e) => {
                self.target.state.then_to_err(Some(e.into()));

                Err(self)
            }
        }
    }

    /**
    Convert this recovery guard into an error.
    */
    pub fn into_error(self) -> PoisonError {
        self.into()
    }
}

impl<'a, T, Target> PoisonRecover<'a, T, Target>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    pub(super) fn new(target: Target) -> PoisonRecover<'a, T, Target> {
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
            .field(&"source", &self.target.state.as_dyn_error())
            .finish()
    }
}

impl<'a, T, Target> fmt::Display for PoisonRecover<'a, T, Target>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.target.state.as_dyn_error(), f)
    }
}

impl<'a, T, Target> AsRef<dyn Error + Send + Sync + 'static> for PoisonRecover<'a, T, Target>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn as_ref(&self) -> &(dyn Error + Send + Sync + 'static) {
        self.target.state.as_dyn_error()
    }
}

impl<'a, T, Target> From<PoisonRecover<'a, T, Target>> for Box<dyn Error + 'static>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn from(guard: PoisonRecover<'a, T, Target>) -> Self {
        guard.target.state.to_dyn_error()
    }
}

impl<'a, T, Target> From<PoisonRecover<'a, T, Target>> for Box<dyn Error + Send + 'static>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn from(guard: PoisonRecover<'a, T, Target>) -> Self {
        guard.target.state.to_dyn_error()
    }
}

impl<'a, T, Target> From<PoisonRecover<'a, T, Target>> for Box<dyn Error + Send + Sync + 'static>
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn from(guard: PoisonRecover<'a, T, Target>) -> Self {
        guard.target.state.to_dyn_error()
    }
}

impl<'a, T, Target> From<PoisonRecover<'a, T, Target>> for PoisonError
where
    Target: ops::Deref<Target = Poison<T>>,
{
    fn from(guard: PoisonRecover<'a, T, Target>) -> Self {
        guard.target.state.to_error()
    }
}
