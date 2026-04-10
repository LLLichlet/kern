# json

`json` is a borrowed JSON frontend for Kern.

The first version is intentionally narrow:

- validates JSON without allocation
- slices a top-level value out of a larger byte stream
- preserves the original raw value bytes
- renders a compact form into caller-provided output buffers
- reports parse failures in terms of byte offsets, with line/column helpers

Current surface:

- `validate(text)` for full-document validation
- `parse_prefix(text)` for streaming or protocol prefixes
- `parse(text)` for a single complete JSON value
- `compact_size(value)` to size compact output before writing
- `render_compact(value, out)` for whitespace-free rendering
- `error_offset(err)` and `locate_error(text, err)` for diagnostics

Current scope is deliberate:

- no allocation-backed DOM yet
- no typed decode/encode layer yet
- no JSON5 or comment support
- no UTF-16 unescaping API yet; strings remain borrowed raw JSON slices

That keeps the first package good at the hardest low-level jobs:
correctness, offset tracking, streaming boundaries, and zero-allocation slicing.
