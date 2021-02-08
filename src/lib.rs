/*!
Utilities for unwind-safety.

This library contains [`Poison<T>`], which can be used to detect when state may be poisoned by
early returns, and to propagate errors and unwinds across threads that share state.

## Detecting invalid state

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
            .or_else(|guard| guard.try_recover_with(|file| Writer::check_and_fix(file)))
            .map_err(|guard| guard.into_error())?;

        let mut scope = Poison::scope(file);

        scope.try_catch_unwind(|file| {
            Writer::write_data_header(file, data.id, data.payload.len() as u64)?;
            Writer::write_data_payload(file, data.payload)?;

            Ok::<(), anyhow::Error>(())
        })?;

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

*/

#![feature(
    async_closure,
    backtrace,
    once_cell,
    arbitrary_self_types,
    try_trait,
    ready_macro
)]

#[macro_use]
extern crate pin_project;

pub mod guard;
pub mod poison;

#[doc(inline)]
pub use self::poison::Poison;

#[cfg(test)]
mod tests;
