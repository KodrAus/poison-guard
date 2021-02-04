# `poison-guard`

A library for sketching out a generic `Poison<T>` type and other unwind-safe functions.

It's a bit like the [`poison`](https://github.com/reem/rust-poison) and [`with_drop`](https://github.com/koraa/with_drop/) crates.

## Use cases

The `Poison<T>` type is intended to be used in synchronization primitives to standardize their poisoning behavior in the presence of unwinds.

For `Mutex`, that means detecting panics that unwind through a lock and giving users a chance to recover from them:

```rust
// In Mutex<T>

let mutex: Mutex<Poison<i32>> = Mutex::new(Poison::new(42));

assert_eq!(42, *mutex.lock().poison().unwrap());
```

For `SyncLazy`, that means catching panics that would cause global initialization to fail and surfacing them:

```rust
// In SyncLazy<T>

static LAZY: SyncLazy<Poison<i32>> = SyncLazy::new(|| Poison::catch_unwind(|| {
    if some_failure_condition {
        panic!("oh no!");
    }

    42
}));

if !LAZY.is_poisoned() {
    assert_eq!(42, *LAZY.get().unwrap());
}
```

The `init_unwind_safe` function can be used to make working with `MaybeUninit` less leaky.

The classic `MaybeUninit` example of initializing an array looks something like this:

```rust
let mut arr: [MaybeUninit<u8>; 16] = unsafe { MaybeUninit::uninit().assume_init() };
let mut i: usize = 0;

for elem in &mut arr[0..16] {
    *elem = MaybeUninit::new(i as u8);
    i += 1;
}

let arr: [u8; 16] = unsafe { arr.assume_init() }
```

Using `init_unwind_safe` it can be rewritten like this to try avoid leaks in case initialization unwinds:

```rust
let arr: [u8; 16] = init_unwind_safe(
    0usize,
    |i, mut uninit| {
        for elem in uninit.array_mut() {
            *elem = mem::MaybeUninit::new(*i as u8);
            *i += 1;
        }

        unsafe { uninit.assume_init() }
    },
    |i, unwound| {
        for elem in &mut unwound.into_array()[0..*i] {
            unsafe {
                ptr::drop_in_place(elem.as_mut_ptr() as *mut u8);
            }
        }
    },
);
```
