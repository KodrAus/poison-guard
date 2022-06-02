/*!
Utilities for maintaining sane state in the presence of panics and failures.

This library contains [`Poison<T>`], which implements poisoning independently of locks or
other mechanisms for sharing state.

## What is poisoning?

Poisoning is a general strategy for keeping state consistent by blocking direct access to
state if a previous user did something unexpected with it.

Poisoning can be applied anywhere there's complex state management at play, but is particularly
useful for cordoning off external resources, like files, that may become corrupted. Rust implements
poisoning in the standard library's `Mutex<T>` type. This library offers poisoning without assuming
locks.

## Detecting invalid state

In simple cases, we can just access a value, and if a panic occurs the value will be poisoned:

```
use poison_guard::Poison;

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
        let mut state = Poison::on_unwind(&mut self.0).unwrap();

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
use poison_guard::Poison;
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
        // Acquire a guard for our state that will only be unpoisoned
        // if we explicitly recover it
        let mut file = Poison::unless_recovered(&mut self.file)
            .or_else(|poisoned| {
                // If the value was poisoned, we'll try recover it
                // This means some past user either panicked or explicitly
                // poisoned the value
                poisoned.try_recover_with(|file| Writer::check_and_fix(file))
            })
            .map_err(|poisoned| poisoned.into_error())?;

        // Now that we have access to the value, we can interact with it
        Writer::write_data_header(&mut file, data.id, data.payload.len() as u64)?;
        Writer::write_data_payload(&mut file, data.payload)?;

        // Return the guard, unpoisoning the value
        // If we early return through a panic or `?` before we get here
        // then the guard will remain poisoned
        Poison::recover(file);

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
use poison_guard::Poison;
use std::{sync::Arc, thread};
use parking_lot::Mutex;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let mutex = Arc::new(Mutex::new(Poison::new(String::from("a value!"))));

// Access the value from another thread, but poison while working with it
# let h = {
# let mutex = mutex.clone();
thread::spawn(move || {
    let mut guard = Poison::on_unwind(mutex.lock()).unwrap();

    guard.push_str("And some more!");

    panic!("explicit panic");
})
# };
# drop(h.join());

// ..

// Later, we try access the poison
// If it was poisoned we'll get a guard that can be
// recovered, unwrapped or converted into an error
match Poison::on_unwind(mutex.lock()) {
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

mod poison;

#[doc(inline)]
pub use self::poison::*;

#[cfg(test)]
mod tests;
