use crate::poison::PoisonError;
use std::{
    error::Error,
    fmt,
    marker,
    ops,
    panic::UnwindSafe,
    thread,
};

use super::Poison;

/**
A guard for a valid value that will unpoison on drop.
*/
pub struct PoisonGuard<'a, T, Target = &'a mut Poison<T>>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    target: Target,
    _marker: marker::PhantomData<&'a mut T>,
}

impl<'a, T, Target> UnwindSafe for PoisonGuard<'a, T, Target> where
    Target: ops::DerefMut<Target = Poison<T>>
{
}

impl<'a, T, Target> PoisonGuard<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    #[track_caller]
    pub(super) fn poison_on_unwind(mut target: Target) -> PoisonGuard<'a, T, Target> {
        target.state.guarded();

        PoisonGuard {
            target,
            _marker: Default::default(),
        }
    }

    #[track_caller]
    pub(super) fn poison_now(mut target: Target) -> PoisonGuard<'a, T, Target> {
        target.state.poison_with_error(None);

        PoisonGuard {
            target,
            _marker: Default::default(),
        }
    }

    #[track_caller]
    pub(super) fn poison_with_error<E>(mut guard: Self, e: E) -> PoisonError
    where
        E: Into<Box<dyn Error + Send + Sync>>,
    {
        guard.target.state.poison_with_error(Some(e.into()));
        guard.target.state.to_error()
    }

    #[track_caller]
    pub(super) fn unpoison_now(mut guard: Self) {
        guard.target.state.unpoison();
    }
}

impl<'a, T, Target> Drop for PoisonGuard<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    #[track_caller]
    fn drop(&mut self) {
        if thread::panicking() {
            self.target.state.poison_with_panic(None);
        } else {
            self.target.state.unpoison_if_guarded();
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
