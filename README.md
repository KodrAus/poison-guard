# `poison-guard`

Utilities for maintaining sane state in the presence of panics and failures.
It's a bit like the [`poison`](https://github.com/reem/rust-poison) and [`with_drop`](https://github.com/koraa/with_drop/) crates.

## Use cases

The `Poison<T>` type is intended to be used in synchronization primitives to standardize their poisoning behavior in the presence of unwinds.

For `Mutex`, that means detecting panics that unwind through a lock and giving users a chance to recover from them:

```rust
// In Mutex<T>

let mutex: Mutex<Poison<i32>> = Mutex::new(Poison::new(42));

assert_eq!(42, *Poison::on_unwind(mutex.lock()).unwrap());
```

For `SyncLazy`, that means catching panics that would cause global initialization to fail and surfacing them:

```rust
// In Lazy<T>

static LAZY: SyncLazy<Poison<i32>> = SyncLazy::new(|| Poison::new_catch_unwind(|| {
    if some_failure_condition {
        panic!("oh no!");
    }

    42
}));

if !LAZY.is_poisoned() {
    assert_eq!(42, *LAZY.get().unwrap());
}
```

`Poison<T>` retains some context about how the guard was originally poisoned for future callers. If a poisoned guard is propagated across threads it offers some better debug information than what you'd get with a plain panic.
