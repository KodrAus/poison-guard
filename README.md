# `poison-guard`

Utilities for maintaining sane state in the presence of panics and failures.
It's a bit like the [`poison`](https://github.com/reem/rust-poison) and [`with_drop`](https://github.com/koraa/with_drop/) crates.

## What is poisoning?

Poisoning is a general strategy for keeping state consistent by blocking direct access to state if a previous user did something unexpected with it.

## Use cases

- You have some external resource, like a `File`, that might become corrupted, and you want to know about it.
- You're sharing some state across threads, and you want any panics that happen while sharing to propagate.
- You're using a non-poisoning container (like `once_cell::sync::Lazy` or `parking_lot::Mutex`) and want to add poisoning to them.

## Getting started

Add `poison-guard` to your `Cargo.toml`:

```toml
[dependencies.poison-guard]
version = "0.1.0"
```

Then wrap your state in a `Poison<T>`:

```rust
use poison_guard::Poison;

pub struct MyData {
    state: Poison<MyState>,
}
```

When you want to access your state, you can acquire a poison guard:

```rust
let mut guard = Poison::on_unwind(&mut my_data.state).unwrap();

do_something_with_state(&mut guard);
```

If a panic unwinds through a poison guard it'll panic the value, blocking future callers from accessing it. Poisoned
values can be recovered, or the original failure can be propagated to those future callers.

For more details, see the documentation.
