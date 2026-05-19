# `std`

`std` is the user-facing standard library.

It provides filesystem, IO, environment, process, message, synchronization,
terminal, testing, and time facilities on top of `base`.

## Boundaries

- `std` may depend on `base`.
- Hosted OS details stay inside `std.host`.
- Runtime startup and compiler-required fallbacks stay in `rt`.
- Freestanding primitives should stay in `base`.

## Concurrency and events

`base.sync` owns freestanding atomics, `SpinLock`, and `Once`. These primitives
busy-wait and stay usable for kernels, boot code, and other no-host contexts.

`std.sync` is the hosted synchronization namespace. It currently re-exports the
freestanding primitives and deliberately does not expose `Thread`, blocking
`Mutex`, `RwLock`, `Condvar`, or channel APIs until Kern has explicit
cross-platform handle, cancellation, and error contracts. Future hosted
blocking primitives should be non-poisoning by default: a panic or abort policy
belongs to the language/runtime boundary, not to a lock silently changing
semantic state.

`std` does not currently expose a cross-platform `Poller`. Readiness and
completion APIs are different enough across epoll, kqueue, IOCP, io_uring, and
direct syscalls that a lowest-common-denominator wrapper would hide important
resource and wakeup semantics. Platform-specific polling should remain explicit
until the standard library has enough users to justify a small, honest common
shape.

Kern should not add language-level `async`/`await` before the runtime,
cancellation, defer/drop, and OS event boundary are stable. Coroutines, fibers,
and direct syscall/C FFI event loops can be explored as libraries first.

## Documentation

From the `library/` workspace root:

```sh
craft doc --project-path std
```
