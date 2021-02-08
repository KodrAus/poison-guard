use std::{
    error::Error,
    fmt,
    future::Future,
    marker, mem,
    ops::{self, Try},
    panic,
    pin::Pin,
    ptr, task, thread,
};

use super::{Poison, PoisonError, PoisonGuard, PoisonRecover, PoisonState};

/**
A scope for a valid value.
*/
// TODO: Is it possible to avoid this type?
// We could just use a `&'b mut PoisonGuard<'a, T, Target>` here
pub struct PoisonScope<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>> + 'a,
{
    target: Target,
    // TODO: Using `UnsafeCell` we could avoid having to stash this here
    // and just work directly off the `Poison` type
    state: PoisonState,
    _marker: marker::PhantomData<&'a mut Poison<T>>,
}

impl<'a, T, Target> PoisonScope<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>> + 'a,
{
    /**
    Use the value within the scope.

    Scopes can be synchronous or asynchronous functions.

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
    pub fn try_catch_unwind<'b, Initial, Continue, Ok>(
        &'b mut self,
        with: Initial,
    ) -> TryCatchUnwind<'b, T, Initial, Continue, Ok>
    where
        Initial: FnOnce(&'b mut T) -> Continue,
    {
        if self.state.is_unpoisoned_or_sentinel() {
            // NOTE: The concrete `&'b mut` lifetime on our closure means a borrow of
            // the inner value can escape the `try_catch_unwind`. So even if it fails,
            // the value can still be accessed afterwards. This isn't _great_, but is
            // necessary for async, where we can't use HRTB. It's not as bad as it sounds
            // though, the borrow still can't escape our borrow of `Poison`, which only guarantees
            // that if we fail then the next caller will receive a poisoned guard.
            // You could think of this borrow escaping a bit like explicitly catching a panic or
            // ignoring a result while holding a poison guard. Within the scope of the `Poison`
            // all bets are off, but when you attempt to enter it again you'll have to deal with
            // whatever mess was left behind.
            TryCatchUnwind(TryCatchUnwindInner::Initial(Some((
                with,
                &mut self.target.value,
                &mut self.state,
            ))))
        } else {
            TryCatchUnwind(TryCatchUnwindInner::Done(Some(Err(self.state.to_error()))))
        }
    }

    /**
    Convert the scope back into a regular poison guard.

    If the scope was poisoned then this will produce a guard to recover.
    */
    pub fn poison(self) -> Result<PoisonGuard<'a, T, Target>, PoisonRecover<'a, T, Target>> {
        let mut target = Self::take(self);
        target.state.then_to_unpoisoned();

        if target.is_poisoned() {
            Err(PoisonRecover::new(target))
        } else {
            Ok(PoisonGuard::new(target))
        }
    }

    pub(super) fn new(target: Target) -> Self {
        let state = target.state.clone();

        PoisonScope {
            target,
            state,
            _marker: Default::default(),
        }
    }

    pub(super) fn take(mut guard: Self) -> Target {
        // Swap the state built back onto the guard
        std::mem::swap(&mut guard.state, &mut guard.target.state);

        let target = &mut guard.target as *mut Target;
        let state = &mut guard.state as *mut PoisonState;

        // Forgetting the struct itself here is ok, because we
        // manually drop the other fields of `PoisonScope`
        mem::forget(guard);

        // SAFETY: The target pointers are still valid
        unsafe { ptr::drop_in_place::<PoisonState>(state) };
        unsafe { ptr::read(target) }
    }
}

impl<'a, T, Target> ops::Drop for PoisonScope<'a, T, Target>
where
    Target: ops::DerefMut<Target = Poison<T>> + 'a,
{
    fn drop(&mut self) {
        if thread::panicking() {
            self.target.state.then_to_panic(None);
        } else {
            // When we drop the scope, swap our captured poison state into
            // the parent `Poison`'s
            std::mem::swap(&mut self.state, &mut self.target.state);
            self.target.state.then_to_unpoisoned();
        }
    }
}

/**
An active poison scope.
*/
#[pin_project]
#[must_use = "poison scopes do nothing unless `?`ed or `await`ed"]
pub struct TryCatchUnwind<'a, T, Initial, Continue, Ok>(
    #[pin] TryCatchUnwindInner<'a, T, Initial, Continue, Ok>,
);

#[pin_project(project = InnerProjection)]
enum TryCatchUnwindInner<'a, T, Initial, Continue, Ok> {
    Initial(Option<(Initial, &'a mut T, &'a mut PoisonState)>),
    Continue(#[pin] Continue, &'a mut PoisonState),
    Done(Option<Result<Ok, PoisonError>>),
}

impl<'a, T, Initial, Continue, Ok> TryCatchUnwind<'a, T, Initial, Continue, Ok>
where
    TryCatchUnwind<'a, T, Initial, Continue, Ok>: Try,
{
    /**
    Map the successful value produced by a scope.
    */
    pub fn map<U>(
        self,
        op: impl FnOnce(<Self as Try>::Ok) -> U,
    ) -> Result<U, <Self as Try>::Error> {
        self.into_result().map(op)
    }

    /**
    Map the error value produced by a scope.
    */
    pub fn map_err<F>(
        self,
        op: impl FnOnce(<Self as Try>::Error) -> F,
    ) -> Result<<Self as Try>::Ok, F> {
        self.into_result().map_err(op)
    }

    /**
    Unwrap the successful value produced by a scope, panicking if it failed.
    */
    pub fn unwrap(self) -> <Self as Try>::Ok
    where
        <Self as Try>::Error: fmt::Debug,
    {
        self.into_result().unwrap()
    }

    /**
    Unwrap the error value produced by a scope, panicking if it succeeded.
    */
    pub fn unwrap_err(self) -> <Self as Try>::Error
    where
        <Self as Try>::Ok: fmt::Debug,
    {
        self.into_result().unwrap_err()
    }

    /**
    Unwrap the successful value produced by a scope, panicking with the given message if it failed.
    */
    pub fn expect(self, msg: &str) -> <Self as Try>::Ok
    where
        <Self as Try>::Error: fmt::Debug,
    {
        self.into_result().expect(msg)
    }

    /**
    Unwrap the error value produced by a scope, panicking with the given message if it succeeded.
    */
    pub fn expect_err(self, msg: &str) -> <Self as Try>::Error
    where
        <Self as Try>::Ok: fmt::Debug,
    {
        self.into_result().expect_err(msg)
    }
}

// Synchronous scoping
impl<'a, T, Initial, Continue, R> Try for TryCatchUnwind<'a, T, Initial, Continue, R>
where
    Initial: FnOnce(&'a mut T) -> Continue,
    Continue: Try<Ok = R>,
    Continue::Error: Into<Box<dyn Error + Send + Sync>>,
{
    type Ok = R;
    type Error = PoisonError;

    #[track_caller]
    fn into_result(self) -> Result<Self::Ok, Self::Error> {
        match self.0 {
            TryCatchUnwindInner::Initial(initial) => {
                let (initial, v, poisoned) = initial.unwrap();

                match panic::catch_unwind(panic::AssertUnwindSafe(move || initial(v))) {
                    Ok(scope) => Self(TryCatchUnwindInner::Continue(scope, poisoned)).into_result(),
                    Err(panic) => {
                        poisoned.then_to_panic(Some(panic));
                        Err(poisoned.to_error())
                    }
                }
            }
            TryCatchUnwindInner::Continue(scope, poisoned) => match scope.into_result() {
                Ok(r) => Ok(r),
                Err(e) => {
                    poisoned.then_to_err(Some(e.into()));
                    Err(poisoned.to_error())
                }
            },
            TryCatchUnwindInner::Done(r) => r.unwrap(),
        }
    }

    fn from_error(e: PoisonError) -> Self {
        TryCatchUnwind(TryCatchUnwindInner::Done(Some(Err(e))))
    }

    fn from_ok(r: R) -> Self {
        TryCatchUnwind(TryCatchUnwindInner::Done(Some(Ok(r))))
    }
}

// Asynchronous scoping
impl<'a, T, Initial, Continue, Ok> Future for TryCatchUnwind<'a, T, Initial, Continue, Ok>
where
    Initial: FnOnce(&'a mut T) -> Continue,
    Continue: Future,
    Continue::Output: Try<Ok = Ok>,
    <Continue::Output as Try>::Error: Into<Box<dyn Error + Send + Sync>>,
{
    type Output = Result<Ok, PoisonError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context) -> task::Poll<Self::Output> {
        let projected = self.as_mut().project();
        let state = projected.0.project();

        match state {
            InnerProjection::Initial(initial) => {
                let (initial, v, poisoned) = initial.take().unwrap();

                match panic::catch_unwind(panic::AssertUnwindSafe(move || initial(v))) {
                    Ok(scope) => {
                        self.set(TryCatchUnwind(TryCatchUnwindInner::Continue(
                            scope, poisoned,
                        )));
                        self.poll(cx)
                    }
                    Err(panic) => {
                        poisoned.then_to_panic(Some(panic));
                        task::Poll::Ready(Err(poisoned.to_error()))
                    }
                }
            }
            InnerProjection::Continue(scope, poisoned) => {
                match panic::catch_unwind(panic::AssertUnwindSafe(|| scope.poll(cx))) {
                    Ok(task::Poll::Pending) => task::Poll::Pending,
                    Ok(task::Poll::Ready(r)) => match r.into_result() {
                        Ok(r) => task::Poll::Ready(Ok(r)),
                        Err(e) => {
                            poisoned.then_to_err(Some(e.into()));
                            task::Poll::Ready(Err(poisoned.to_error()))
                        }
                    },
                    Err(panic) => {
                        poisoned.then_to_panic(Some(panic));
                        task::Poll::Ready(Err(poisoned.to_error()))
                    }
                }
            }
            InnerProjection::Done(r) => task::Poll::Ready(r.take().unwrap()),
        }
    }
}
