/*!
Utilities for unwind-safety.
*/

#![feature(backtrace, once_cell, arbitrary_self_types)]

pub mod guard;
pub mod poison;

#[doc(inline)]
pub use self::poison::Poison;

#[cfg(test)]
mod tests;
