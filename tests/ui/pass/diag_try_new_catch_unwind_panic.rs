use std::{error::Error, io, iter};

use poison_guard::Poison;

fn run() -> Result<(), Box<dyn Error + 'static>> {
    let mut p = Poison::try_new_catch_unwind(|| {
        let mut v = 42;

        v += 1;

        if v > 10 {
            panic!("explicit panic");
        }

        Ok::<i32, io::Error>(v)
    });

    let g = Poison::on_unwind(&mut p)?;

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

    for err in iter::successors(err.source(), |&err| err.source()) {
        println!("  caused by: {}", err);
    }
}
