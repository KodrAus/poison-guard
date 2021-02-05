#![feature(backtrace, once_cell, arbitrary_self_types)]

pub mod guard;
pub mod poison;

#[cfg(test)]
mod tests;
