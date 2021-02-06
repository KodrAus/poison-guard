#![feature(backtrace, once_cell)]

use std::{iter, error::Error, panic, lazy::SyncLazy as Lazy};

use poison_guard::Poison;

static LAZY: Lazy<Poison<i32>> = Lazy::new(|| Poison::new_catch_unwind(|| panic!("explicit panic")));

fn run() -> Result<(), Box<dyn Error + 'static>> {
    let g = LAZY.get()?;

    assert_eq!(42, *g);

    Ok(())
}

fn main() {
    let err = run().unwrap_err();

    render(&*err);
}

fn render(err: &(dyn Error + 'static)) {
    println!("debug: {:?}", err);
    println!();

    println!("{}", err);
    if let Some(bt) = err.backtrace() {
        println!("{}", bt);
    }

    for err in iter::successors(err.source(), |&err| err.source()) {
        println!("  caused by: {}", err);
        if let Some(bt) = err.backtrace() {
            println!("{}", bt);
        }
    }
}
