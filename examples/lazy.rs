use once_cell::sync::Lazy;
use poison_guard::Poison;

static LAZY: Lazy<Poison<i32>> = Lazy::new(|| Poison::new_catch_unwind(|| 42));

fn main() {
    match LAZY.get() {
        // If initialization succeeded then print the value
        Ok(v) => println!("{}", v),
        // If initialization failed then print the error
        Err(e) => println!("{}", e),
    }
}
