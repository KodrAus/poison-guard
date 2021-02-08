use std::{fmt, marker, mem, ops, ptr, thread};

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

impl<'a, T, Target> PoisonGuard<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>>,
{
    #[track_caller]
    pub(super) fn new(mut target: Target) -> PoisonGuard<'a, T, Target> {
        target.state.then_to_sentinel();

        PoisonGuard {
            target,
            _marker: Default::default(),
        }
    }

    pub(super) fn take(mut guard: Self) -> Target {
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
            self.target.state.then_to_panic(None);
        } else {
            self.target.state.then_to_unpoisoned();
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
