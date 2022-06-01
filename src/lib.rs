/*!
Utilities for unwind-safety.

This library contains [`Poison<T>`], which can be used to detect when state may be poisoned by
early returns, and to propagate errors and unwinds across threads that share state.

## Detecting invalid state

In simple cases, we can just access a value, and if a panic occurs the value will be poisoned:

```
# use poison_guard::Poison;
struct Account(Poison<AccountState>);

struct AccountState {
    // The total _must_ be the sum of all changes
    total: i64,
    changes: Vec<i64>,
}

impl Account {
    pub fn new() -> Self {
        Account(Poison::new(AccountState { total: 0, changes: vec![] }))
    }

    pub fn push_change(&mut self, change: i64) {
        let mut state = self.0.as_mut().poison().unwrap();

        state.changes.push(change);

        // If we panic here, our state will poison
        // The total won't be the sum of all changes, but that's ok
        // Future callers won't be able to access the state without
        // attempting to recover it first

        state.total = state.changes.iter().copied().sum();
    }

    pub fn total(&self) -> i64 {
        self.0.get().unwrap().total
    }
}
```

Say we're writing data to a file. If an individual write fails we might not know exactly what state
the file has been left in on-disk and need to recover it before accessing again:

```
# use poison_guard::Poison;
use anyhow::Error;
use std::{io::{self, Write}, fs::File};

struct Writer {
    file: Poison<File>,
}

struct Data {
    id: u64,
    payload: Vec<u8>,
}

impl Writer {
    pub fn write_data(&mut self, data: Data) -> anyhow::Result<()> {
        let file = self
            .file
            .as_mut()
            .poison()
            .or_else(|guard| {
                // If the value was poisoned, we'll try recover it
                // This means some past user either panicked or explicitly
                // poisoned the value
                guard.try_recover_with(|file| Writer::check_and_fix(file))
            })
            .map_err(|guard| guard.into_error())?;

        // Now that we have access to the value, we can create a scope over it
        // Within this scope, any panic or early return from `?` will poison the value
        let mut scope = Poison::scope(file);

        scope.try_catch_unwind(|file| {
            Writer::write_data_header(file, data.id, data.payload.len() as u64)?;
            Writer::write_data_payload(file, data.payload)?;

            Ok::<(), anyhow::Error>(())
        })?;

        // Let the guard fall out of scope, this will unpoison the value so future
        // callers can come along and use it
        Ok(())
    }

    fn check_and_fix(file: &mut File) -> anyhow::Result<()> {
        // ..
#       let _ = file;
#       Ok(())
    }

    fn write_data_header(file: &mut File, id: u64, len: u64) -> anyhow::Result<()> {
        // ..
#       let _ = (file, id, len);
#       Ok(())
    }

    fn write_data_payload(file: &mut File, payload: Vec<u8>) -> anyhow::Result<()> {
        // ..
#       let _ = (file, payload);
#       Ok(())
    }
}
```

## Propagating errors and unwinds

If a `Poison<T>` is poisoned, future attempts to access it may convert that into a panic or error:

```should_panic
# use poison_guard::Poison;
# use std::{sync::Arc, thread};
# use parking_lot::Mutex;
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let mutex = Arc::new(Mutex::new(Poison::new(String::from("a value!"))));

// Access the value from another thread, but poison while working with it
# let h = {
# let mutex = mutex.clone();
thread::spawn(move || {
    let mut guard = mutex.lock().poison().unwrap();

    guard.push_str("And some more!");

    panic!("explicit panic");
})
# };
# drop(h.join());

// ..

// Later, we try access the poison
// If it was poisoned we'll get a guard that can be
// recovered, unwrapped or converted into an error
match mutex.lock().poison() {
    Ok(guard) => {
        println!("the value is: {}", &*guard);
    }
    Err(recover) => {
        println!("{}", recover);

        return Err(recover.into_error().into());
    }
}
# Ok(())
# }
```

The above example will output something like:

```text
poisoned by a panic (the poisoning guard was acquired at 'src/lib.rs:13:38')
```
*/

pub mod guard;
pub mod poison;

#[doc(inline)]
pub use self::poison::Poison;

#[cfg(test)]
mod tests;
