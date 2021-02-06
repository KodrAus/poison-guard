/*!
Unwind-safe initialization and cleanup.

## Is this just `try`/`catch`?

Absolutely not, where did you learn such words?
Once an exception has been caught, `catch` gives you tools to determine how its propagated.
You may choose to ignore, rethrow, or repackage it.
These functions don't let you catch an unwind that occurs, just run some extra code along the way.

## Is this just `try`/`finally` then?

That's a little closer, but still not quite there.
A `finally` block executes on both normal and exceptional paths, where the unwind closures here only execute after an unwind.
*/

use std::{
    cell::UnsafeCell,
    mem::{self, MaybeUninit},
    ops, ptr,
};

/**
Attempt to initialize a value that may unwind.

The initialization function will be called to produce a value, `T`, from a `MaybeUninit<T>`.
If the initialization function unwinds, then the unwind function will be called.
This gives the caller a chance to clean up any partially initialized state and avoid leaks.

The state value is shared between initialization and unwinding so it can be used to determine
what was and wasn't initialized.

If the unwind function panics then it may trigger an abort.

`init_unwind_safe` guarantees that `Drop` won't be called on `T` (barring any use of `mem::{take, swap, replace}`)
if the `on_unwind` closure executes.
*/
pub fn init_unwind_safe<S, T>(
    state: S,
    init: impl for<'state, 'init> FnOnce(&'state mut S, MaybeUninitSlot<'init, T>) -> InitSlot<'init, T>,
    on_unwind: impl for<'state> FnOnce(&'state mut S, UnwoundSlot<T>),
) -> T {
    // The value to initialize and state are stored in `UnsafeCell`s
    // They're shared between the drop impl for a guard and the init closure
    // Only one of these sources can access the values at a time
    let uninit = UnsafeCell::new(Some(MaybeUninit::<T>::uninit()));
    let state = UnsafeCell::new(state);

    let guard = InitGuard(&state, &uninit, Some(on_unwind));

    // Run the initialization function
    let init = init(
        // SAFETY: These exclusive accesses to the inner value and state doesn't overlap a borrow given to the unwind closure
        // These borrows expire _before_ the unwind closure gets a chance to run
        unsafe { &mut *state.get() },
        MaybeUninitSlot(unsafe { &mut *uninit.get() }),
    );

    // Ensure the guard hasn't been swapped
    // TODO: This check is a reminder to think about implications of `mem::swap` and friends
    // This _shouldn't_ be possible since we have an arbitrarily small invariant lifetime, but is worth trying to break
    assert_eq!(&uninit as *const _, (init.0).0 as *mut _ as *const _);

    // Drop the unwind guard
    // This happens in a specific order:
    // - First, ensure the uninitialized state is `None`, this prevents the `on_unwind` closure from running
    // - Next, drop the guard (which won't then do any work, but will drop the `on_unwind` closure)
    // - Finally, drop the state value after the guard has had a chance to access it

    let value = InitSlot::into_inner(init);

    // Dropping the guard here will never panic, but dropping the state might
    // If that happens we unwind regularly, since the value is fully initialized
    drop(guard);
    drop(state);

    value
}

/**
Attempt to initialize a value that may fail or unwind.

The initialization function will be called to try produce a value, `T`, from a `MaybeUninit<T>`.
If the initialization function fails or unwinds, then the unwind function will be called.
This gives the caller a chance to clean up any partially initialized state and avoid leaks.

The state value is shared between initialization and unwinding so it can be used to determine
what was and wasn't initialized.

If the unwind function panics then it may trigger an abort.

`try_init_unwind_safe` guarantees that `Drop` won't be called on `T` (barring any use of `mem::{take, swap, replace}`)
if the `on_err_unwind` closure executes.
*/
pub fn try_init_unwind_safe<S, T, E>(
    state: S,
    try_init: impl for<'state, 'init> FnOnce(
        &'state mut S,
        MaybeUninitSlot<'init, T>,
    ) -> Result<InitSlot<'init, T>, E>,
    on_err_unwind: impl for<'state> FnOnce(&'state mut S, UnwoundSlot<T>),
) -> Result<T, E> {
    // The value to initialize and state are stored in `UnsafeCell`s
    // They're shared between the drop impl for a guard and the init closure
    // Only one of these sources can access the values at a time
    let uninit = UnsafeCell::new(Some(MaybeUninit::<T>::uninit()));
    let state = UnsafeCell::new(state);

    let guard = InitGuard(&state, &uninit, Some(on_err_unwind));

    // Run the initialization function
    match try_init(
        // SAFETY: These exclusive accesses to the inner value and state doesn't overlap a borrow given to the unwind closure
        // These borrows expire _before_ the unwind closure gets a chance to run
        unsafe { &mut *state.get() },
        MaybeUninitSlot(unsafe { &mut *uninit.get() }),
    ) {
        Ok(init) => {
            // Drop the unwind guard
            // This happens in a specific order:
            // - First, ensure the uninitialized state is `None`, this prevents the `on_err_unwind` closure from running
            // - Next, drop the guard (which won't then do any work, but will drop the `on_err_unwind` closure)
            // - Finally, drop the state value after the guard has had a chance to access it

            // Ensure the guard hasn't been swapped
            // TODO: This check is a reminder to think about implications of `mem::swap` and friends
            // This _shouldn't_ be possible since we have an arbitrarily small invariant lifetime, but is worth trying to break
            assert_eq!(&uninit as *const _, (init.0).0 as *mut _ as *const _);

            let value = InitSlot::into_inner(init);

            // Dropping the guard here will never panic, but dropping the state might
            // If that happens we unwind regularly, since the value is fully initialized
            drop(guard);
            drop(state);

            Ok(value)
        }
        Err(e) => {
            // Drop the unwind guard
            // Since initialization failed this will execute the unwind closure before returning the error

            drop(guard);
            drop(state);

            Err(e)
        }
    }
}

/**
A potentially uninitialized value.

This type is a wrapper around a `MaybeUninit<T>`.
*/
pub struct MaybeUninitSlot<'a, T>(&'a mut Option<MaybeUninit<T>>);

/**
An initialized value.

This is the result of initializing a `MaybeUninitSlot`.
*/
pub struct InitSlot<'a, T>(MaybeUninitSlot<'a, T>);

impl<'a, T> InitSlot<'a, T> {
    fn into_inner(slot: Self) -> T {
        // SAFETY: An `InitSlot` can only be created from an initialized value
        unsafe { (slot.0).0.take().unwrap().assume_init() }
    }
}

impl<'a, T> ops::Deref for InitSlot<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: An `InitSlot` can only be created from an initialized value
        unsafe { &*self.0.get().as_ptr() }
    }
}

impl<'a, T> ops::DerefMut for InitSlot<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: An `InitSlot` can only be created from an initialized value
        unsafe { &mut *self.0.get_mut().as_mut_ptr() }
    }
}

impl<'a, T> MaybeUninitSlot<'a, T> {
    fn get(&self) -> &MaybeUninit<T> {
        self.0.as_ref().unwrap()
    }

    /**
    Get a reference to the value to initialize.
    */
    pub fn get_mut(&mut self) -> &mut MaybeUninit<T> {
        self.0.as_mut().unwrap()
    }

    /**
    Initialize the value.

    Any previously initialized state will be overwritten without being dropped.
    */
    pub fn init(self, value: T) -> InitSlot<'a, T> {
        *self.0.as_mut().unwrap() = MaybeUninit::new(value);
        InitSlot(self)
    }

    /**
    Consider the value fully initialized.

    This has the same safety requirements as `MaybeUninit::assume_init`.
    */
    pub unsafe fn assume_init(self) -> InitSlot<'a, T> {
        InitSlot(self)
    }
}

impl<'a, T, const N: usize> MaybeUninitSlot<'a, [T; N]> {
    /**
    Get a reference to the value to initialize as an array.
    */
    pub fn array_mut(&mut self) -> &mut [MaybeUninit<T>; N] {
        // SAFETY: `MaybeUninit<T>` has the same layout as `T`
        unsafe {
            mem::transmute::<&mut mem::MaybeUninit<[T; N]>, &mut [mem::MaybeUninit<T>; N]>(
                self.get_mut(),
            )
        }
    }
}

/**
A potentially initialized value.

This type is a wrapper around a `MaybeUninit<T>`.
It's up to the caller to figure out how to drop any partially initialized state in the value.
*/
pub struct UnwoundSlot<T>(MaybeUninit<T>);

impl<T> UnwoundSlot<T> {
    /**
    Take the partially initialized value.
    */
    pub fn into_inner(self) -> MaybeUninit<T> {
        self.0
    }
}

impl<T, const N: usize> UnwoundSlot<[T; N]> {
    /**
    Take the partially initialized value as an array.
    */
    pub fn into_array(mut self) -> [MaybeUninit<T>; N] {
        // SAFETY: `MaybeUninit<T>` has the same layout as `T`
        unsafe {
            ptr::read(&mut self.0 as *mut mem::MaybeUninit<[T; N]> as *mut [mem::MaybeUninit<T>; N])
        }
    }
}

struct InitGuard<'a, S, T, F>(
    &'a UnsafeCell<S>,
    &'a UnsafeCell<Option<MaybeUninit<T>>>,
    Option<F>,
)
where
    F: FnOnce(&mut S, UnwoundSlot<T>);

impl<'a, S, T, F> ops::Drop for InitGuard<'a, S, T, F>
where
    F: FnOnce(&mut S, UnwoundSlot<T>),
{
    fn drop(&mut self) {
        // SAFETY: This exclusive access to the inner value doesn't overlap a borrow given to the init closure
        // It's run in the drop impl of this guard _after_ the init closure has returned or unwound
        if let Some(unwound) = unsafe { &mut *self.1.get() }.take() {
            // SAFETY: This exclusive access to the state doesn't overlap a borrow given to the init closure
            let state = unsafe { &mut *self.0.get() };

            (self.2.take().unwrap())(state, UnwoundSlot(unwound));
        }
    }
}

pub fn drop_unwind_safe<T>(_drop: impl FnOnce(&mut T), _on_unwind: impl FnOnce(&mut T)) -> T {
    unimplemented!("try drop the value, resume on unwind")
}
