# bitio

`bitio` is a small Kern library for low-level bit, byte-order, and masking
helpers.

The package is intentionally narrow:

- no allocator requirements
- no libc requirements
- borrowed input/output only
- suitable as a dependency of protocol, crypto, and codec libraries

Current surface:

- big-endian and little-endian integer read/write helpers
- MSB-first `BitReader` with bit counting and byte-alignment helpers
- MSB-first `BitWriter` with zero-padding alignment helpers
- common rotate helpers
- 4-byte XOR masking helper with an internal `u8x16` fast path
- explicit error reporting for EOF, short output buffers, and invalid bit counts

Current scope is deliberate:

- the public API stays on ordinary borrowed buffers (`[]u8`, `[]mut u8`)
- `^T` / `^mut T` helpers are intentionally omitted for now because this package
  is aimed at protocol and codec buffers, not MMIO-facing volatile memory
- SIMD stays an implementation detail for now; the package uses it where it
  helps bulk byte transforms without exposing a SIMD-specific public surface
