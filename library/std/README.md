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

`std.sync` is the hosted synchronization namespace. It re-exports the
freestanding primitives and provides no-libc blocking `Mutex`, `RwLock`,
`Condvar`, `Channel`, and low-level joinable `Thread` primitives. Blocking
waits use direct OS wait/wake facilities where the platform exposes a stable
kernel or hosted ABI. `Thread` keeps the ABI contract explicit: callers provide
an allocator, stack size, thin entry function, raw context pointer, and must
join exactly once with the same allocator. It does not accept capturing
closures, hidden runtime state, or detached cleanup. `THREAD_MIN_STACK_SIZE`
defines the portable minimum accepted by `std.sync.spawn`.

These primitives are non-poisoning: a panic or abort policy belongs to the
language/runtime boundary, not to a lock silently changing semantic state.
Darwin threading is currently isolated behind a platform-specific fallback
until a stricter no-libc strategy is validated on Darwin CI or hardware.

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
