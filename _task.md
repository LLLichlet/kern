# Kern stdlib work queue

This is a temporary maintenance checklist for stdlib/runtime direction work.
Keep it until the whole round is finished, then delete it in a cleanup commit.

## 1. Buffered I/O and flush

- [x] Review the current `base.io.Write` / `Read` contracts.
- [x] Decide whether `flush` returns `bool`, `void!Error`, or a richer I/O error.
- [x] Add best-effort flush to writer-facing traits without weakening no-std/base users.
- [x] Add `BufWriter` with explicit buffer ownership and predictable flush behavior.
- [x] Add `BufReader` for repeated small reads.
- [x] Add `LineWriter` only if it remains small and useful.
- [x] Distinguish best-effort stream flush from durable file sync APIs.
- [x] Add kernlib tests for buffered writes and explicit flush.

## 2. Internal COW for value-like stdlib types

- [ ] Audit current `String`, path, and byte-buffer APIs for clone/copy pressure.
- [ ] Decide which types should own internal borrowed/static/owned states.
- [ ] Keep the common API surface unified; avoid making users choose between
      owned and COW variants in normal code.
- [ ] Preserve transparent mutation semantics by materializing borrowed/static
      storage only on write.
- [ ] Keep `List[T]` as a simple owned contiguous container unless a real need
      appears.
- [ ] Add tests for borrowed-to-owned materialization, static string reuse, and
      mutation after borrowing.
- [ ] Document the performance model in the type/module comments.

## 3. Synchronous concurrency foundation

- [ ] Review existing atomic and sync support.
- [ ] Design small cross-platform primitives: thread, mutex, rwlock, condvar,
      once, and maybe channel.
- [ ] Keep the base layer freestanding where possible; place hosted OS bindings
      in `std`.
- [ ] Add tests for basic synchronization and poisoning/non-poisoning policy if
      applicable.

## 4. OS event and polling boundary

- [ ] Decide whether stdlib should expose a minimal `Poller` abstraction.
- [ ] Separate readiness/completion APIs from language-level async syntax.
- [ ] Keep platform details explicit: epoll/kqueue/IOCP/io_uring/system calls.
- [ ] Ensure direct syscall/C FFI use remains possible for OS and freestanding
      users.

## 5. Async/coroutine design discussion

- [ ] Decide whether Kern needs language-level async at all.
- [ ] Evaluate library-level coroutines/fibers separately from OS async I/O.
- [ ] Consider whether stackful context switching belongs in std, rt, or a
      third-party package.
- [ ] Define cancellation/defer/drop/resource semantics before any syntax.
- [ ] Do not add `async`/`await` syntax until the runtime and OS boundary model
      is stable.
