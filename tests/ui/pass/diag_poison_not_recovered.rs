use std::{error::Error, iter};

use poison_guard::Poison;

fn run() -> Result<(), Box<dyn Error + 'static>> {
    let mut p = Poison::new(42);

    let g = Poison::unless_recovered(&mut p)?;
    drop(g);

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
