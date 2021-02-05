#![feature(backtrace)]

use std::{iter, io, error::Error};

use poison_guard::Poison;

fn run() -> Result<(), Box<dyn Error + 'static>> {
    let mut p = Poison::try_catch_unwind(|| {
        Err::<i32, io::Error>(io::Error::new(io::ErrorKind::Interrupted, "an IO error"))
    });

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
