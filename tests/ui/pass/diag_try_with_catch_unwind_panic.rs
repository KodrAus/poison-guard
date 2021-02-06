#![feature(backtrace)]

use std::{iter, error::Error, panic};

use poison_guard::Poison;

fn run() -> Result<(), Box<dyn Error + 'static>> {
    let mut p = Poison::new(42);

    let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let mut g = Poison::upgrade(p.as_mut().poison().unwrap());

        *g += 1;

        panic!("explicit panic");
    }));

    let g = p.as_mut().poison()?;

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
