use std::{
    iter,
    sync::Arc,
    thread,
};

use parking_lot::Mutex;
use poison_guard::Poison;

#[derive(Default)]
pub struct Account {
    txns: Vec<i32>,
    // Invariant: the `total` must always be the sum of `txns`
    total: i32,
}

fn main() {
    let shared = Arc::new(Mutex::new(Poison::new(Account::default())));

    // Spawn off some handles that will all work with the shared state
    let handles: Vec<_> = iter::repeat_with(|| {
        let shared = shared.clone();

        thread::spawn(move || {
            let mut account = Poison::on_unwind(shared.lock()).unwrap();

            account.txns.push(3);

            // Eventually we'll poison the shared state
            if account.txns.len() > 3 {
                panic!("too many things happening");
            }

            account.total += 3;
        })
    })
    .take(5)
    .collect();

    // Wait for our handles
    for handle in handles {
        let _ = handle.join();
    }

    let acc = match Poison::on_unwind(shared.lock()) {
        // If the account is valid then we can use it directly
        Ok(acc) => acc,
        // If the account is not valid then we'll need to recover it
        // Our invariant is that the total is always the sum of transactions so that's what we'll fix
        Err(recover) => recover.recover_with(|acc| {
            acc.total = acc.txns.iter().sum();
        }),
    };

    assert_eq!(acc.total, acc.txns.iter().sum());
}
