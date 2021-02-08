#![feature(backtrace)]

use std::{iter, io, error::Error};

use poison_guard::Poison;

fn run() -> Result<(), Box<dyn Error + 'static>> {
    let mut p = Poison::new(42);

    let mut s = Poison::scope(p.as_mut().poison().unwrap());

    s.try_catch_unwind(|g| {
        *g += 1;

        Err::<(), io::Error>(io::Error::new(io::ErrorKind::Interrupted, "an IO error"))
    })?;

    let g = s.poison()?;

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
