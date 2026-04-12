# json

`json` starts with a borrowed JSON frontend for Kern, then adds explicit decode
and materialization paths where the algorithm benefits from owning data.

The first version is intentionally narrow:

- validates JSON without allocation
- slices a top-level value out of a larger byte stream
- preserves the original raw value bytes
- renders a compact form into caller-provided output buffers
- reports parse failures in terms of byte offsets, with line/column helpers

Current surface:

Recommended library-style owning path:

- `value.to_document()` and `value.to_indexed_object_document()` for allocator-hidden owning conversion when you want the JSON package to manage document memory itself
- `parse_owned_document(text)` and `clone_owned_document(value)` for the normal owned document path
- `parse_indexed_object_document(text)` and `clone_indexed_object_document(value)` for the common top-level-object plus indexed-lookup path
- `OwnedDocument.root()` for a read-only `DocumentValue`, `OwnedDocument.root_mut()` for a document-bound `DocumentValueMut`, and `OwnedDocument.deinit()` for tearing the whole document down in one step
- `DocumentValue` for high-level read-only traversal over a document-owned tree: scalar decode, field and index traversal, duplicate-field inspection, required-field schema reads, and compact render without dropping to raw owned pointers
- `DocumentValueMut` for library-style in-document work without allocator plumbing: kind checks, scalar decode, field and index traversal, duplicate-field inspection, required-field schema reads, compact render, `replace`, `reserve_object_fields`, `append_object_field/string/number`, `set_object_field`, `remove_object_field`, `clear_object_fields`, `reserve_array_items`, `push_array_item/string/number`, `pop_array_item`, `remove_array_index`, and `clear_array_items`
- `DocumentObjectEntryRef` and `DocumentObjectEntry` for read-only and mutable document-bound object entry results, so duplicate-key and field-oriented code can stay inside the document API instead of falling back to raw owned pointers
- `IndexedObjectDocument.root()` for a read-only `DocumentValue`, `IndexedObjectDocument.view()` for an `IndexedDocumentView`, and `IndexedObjectDocument.deinit()` for an immutable arena-backed indexed object document that keeps the root object and its index consistent
- `IndexedDocumentView` for high-level indexed object lookup over an indexed document without exposing raw `IndexedObjectView` or `*OwnedValue` pointers in the common read path

Borrowed zero-allocation path:

- `validate(text)` for full-document validation
- `parse_prefix(text)` for streaming or protocol prefixes
- `parse(text)` for a single complete JSON value
- `value.is_null()/is_bool()/is_number()/is_string()/is_array()/is_object()` for fast kind checks
- `value.bool_value()`, `value.number_text()`, `value.string_raw()`, and `value.string_text()` for borrowed scalar access
- `value.u64_value()`, `value.u32_value()`, `value.u16_value()`, `value.u8_value()`, `value.i64_value()`, `value.i32_value()`, `value.i16_value()`, `value.i8_value()`, `value.usize_value()`, and `value.isize_value()` for explicit integer decoding
- `value.f64_value()` and `value.f32_value()` for explicit floating-point decoding
- `value.decoded_string_size()`, `value.write_decoded_string(out)`, and `value.clone_decoded_string(alloc)` for explicit string unescaping
- `value.array_cursor()` and `value.object_cursor()` for borrowed container traversal
- `value.get_index(i)` and `value.get_field(name)` for common lookup paths
- `value.get_bool_field/get_number_text_field/get_string_raw_field/get_string_text_field/get_u64_field/.../get_f32_field` and matching `get_*_index` helpers for optional borrowed schema reads that preserve parse/decode errors but return `None` for missing, out-of-range, or wrong-kind access
- `value.get_last_field(name)`, `value.count_fields(name)`, and `value.collect_fields(name, out)` for explicit duplicate-key handling on borrowed objects
- `value.require_field(name)`, `value.require_bool_field(name)`, `value.require_string_raw_field(name)`, `value.require_string_text_field(name)`, `value.require_decoded_string_size_field(name)`, `value.write_required_decoded_string_field(name, out)`, `value.clone_required_decoded_string_field(name, alloc)`, and typed numeric helpers from `require_u64_field` through `require_f32_field` for required borrowed object-field schema access
- `value.require_index(i)`, `value.require_bool_index(i)`, `value.require_string_raw_index(i)`, `value.require_string_text_index(i)`, `value.require_decoded_string_size_index(i)`, `value.write_required_decoded_string_index(i, out)`, `value.clone_required_decoded_string_index(i, alloc)`, and typed numeric helpers from `require_u64_index` through `require_f32_index` for required borrowed array-element schema access
- `entry.decoded_key_size()`, `entry.write_decoded_key(out)`, and `entry.clone_decoded_key(alloc)` for explicit object-key decoding
- borrowed `ObjectEntry` also forwards common value helpers such as `bool_value`, `number_text`, `string_raw`, `string_text`, decoded-string helpers, and typed numeric decode so field-oriented code does not need to unwrap through `entry.value` manually
- `value.array_len()`, `value.object_len()`, and `value.has_field(name)` for common container metadata checks
- `OwnedObject.get_last_field/count_fields/collect_fields`, matching `OwnedValue` wrappers, and the same duplicate-key queries on `IndexedObjectView`
- `OwnedObject.require_value/require_bool/require_string/require_array/require_object/require_u64/.../require_f32`, matching `OwnedValue` wrappers, and the same required-unique field helpers on `IndexedObjectView`
- `FieldDecodeError` distinguishes missing fields, duplicate fields, wrong JSON kind, and numeric decode failures for schema-style access
- `BorrowedFieldDecodeError` and `IndexDecodeError` do the same for borrowed object fields and array indexes, while also preserving parse and string-decode errors
- `parse_u64_raw(raw)`, `parse_u32_raw(raw)`, `parse_u16_raw(raw)`, `parse_u8_raw(raw)`, `parse_i64_raw(raw)`, `parse_i32_raw(raw)`, `parse_i16_raw(raw)`, `parse_i8_raw(raw)`, `parse_usize_raw(raw)`, and `parse_isize_raw(raw)` for raw integer decoding when you already have a number token
- `parse_f64_raw(raw)` and `parse_f32_raw(raw)` for raw floating-point decoding when you already have a number token
- `compact_size(value)` to size compact output before writing
- `render_compact(value, out)` for whitespace-free rendering
- `error_offset(err)` and `locate_error(text, err)` for diagnostics

Lower-level explicit owning path:

- these APIs remain public for algorithms that intentionally manage raw storage strategy themselves, but they are not the recommended default library-style path
- `value.clone_owned(alloc)` for explicit allocator-driven materialization into `OwnedValue`
- `OwnedValue` / `OwnedObject` / `OwnedArray` helpers for kind checks, scalar access, typed number decode, index lookup, and field lookup on the materialized tree
- `OwnedObject.get_bool/get_string/get_number_text/get_u64/.../get_f64` and matching `OwnedValue` object helpers for convenient field-oriented access on materialized objects
- `OwnedObject.get_array/get_object`, matching `OwnedValue` wrappers, and the same optional nested-container helpers on `IndexedObjectView` for field-oriented traversal without forcing required-field semantics
- `OwnedArray.push/push_string/push_number_text/pop/remove` and `OwnedObject.append/append_string/append_number_text/set/remove_field` for explicit DOM construction and mutation
- `OwnedValue.append_object_field/set_object_field/remove_object_field` and `OwnedValue.push_array_item/push_array_string/push_array_number_text/pop_array_item` for mutating nested materialized values without exposing raw payload internals
- `OwnedValue.compact_size()`, `OwnedValue.render_compact(out)`, `compact_size_owned(value)`, and `render_compact_owned(value, out)` for compact JSON output from the materialized tree
- `OwnedObject.build_indexed_view(alloc)` and `OwnedValue.build_indexed_object_view(alloc)` for an explicit lookup index over a materialized object while preserving entry order in the base representation; the view borrows the materialized storage and owns only its index tables
- `OwnedObject.get_last_field/count_fields/collect_fields`, matching `OwnedValue` wrappers, and the same duplicate-key queries on `IndexedObjectView`
- `OwnedObject.require_value/require_bool/require_string/require_array/require_object/require_u64/.../require_f32`, matching `OwnedValue` wrappers, and the same required-unique field helpers on `IndexedObjectView`

Current scope is deliberate:

- owning DOM exists and now supports explicit construction, replacement, removal, compact rendering, duplicate-key inspection, and broad typed required-field schema access; borrowed object/array schema reads cover the same core scalar families, but higher-level builder ergonomics are still narrower than mature industrial libraries
- no float encode layer yet
- no JSON5 or comment support
- no full typed string/object-key decode layer yet beyond explicit string and key helpers
- `string_text()` removes outer quotes only; escape sequences remain borrowed and verbatim
- integer decode is explicit and strict: non-numbers return `None`, while decimal/exponent forms and overflow return typed errors
- float decode is explicit too: non-numbers return `None`, malformed numbers preserve parse errors, and oversized finite literals report overflow
- owning materialization currently decodes strings and object keys into `String`, preserves object order, and keeps numbers as owned source text
- base owning lookup remains linear for objects; indexed lookup is available as a separate explicit view when the algorithm benefits from it
- indexed object view is an explicit extra layer on top of `OwnedObject`; it keeps first-match semantics for duplicate keys and uses hashing only to narrow candidate entries
- `IndexedObjectDocument` is intentionally immutable at the document surface; if you need to mutate the tree, use `OwnedDocument` or raw owned values and rebuild the index explicitly afterwards
- decoded string helpers perform JSON escape decoding and can materialize into `String` when that is the right algorithmic tradeoff
- object keys expose both `key_raw` and `key_text`; escape sequences are preserved verbatim

That keeps the first package good at the hardest low-level jobs:
correctness, offset tracking, streaming boundaries, and zero-allocation slicing.

Performance notes:

- borrowed validation, slicing, and lookup stay allocation-free; prefer that layer when you do not need an owned tree
- low-level `OwnedValue` materialization still accepts an explicit allocator when you intentionally want to control raw storage strategy yourself
- for library-style use, `OwnedDocument` is the intended higher-level default: it hides per-node allocation details and internalizes document storage strategy instead of asking callers to choose one
- once you are on `OwnedDocument`, prefer `root_mut()` plus `DocumentValueMut` for normal tree updates instead of dropping back to allocator-explicit `OwnedValue` mutation APIs
- if your schema is a top-level object and you know you want repeated field lookup, `IndexedObjectDocument` is the direct high-level path; it materializes and indexes in one arena-backed step instead of bouncing through general-purpose per-node frees
- if the owned tree is phase-local scratch state, an arena allocator is often a better fit than a general-purpose allocator because you can bulk-reset the whole phase instead of paying per-node frees
- the benchmark example supports both the built-in sample and external corpora: `bench_json 120000 clone_arena`, `bench_json 2000 parse bench/corpus/twitter.json`, `bench_json 2000 materialize bench/corpus/citm_catalog.json`, or `bench_json 2000 parse -` for stdin
- benchmark output now reports both `ops_per_sec` and `bytes_per_sec`, so standard corpus runs can be compared against mature libraries in the same units
- `scripts/fetch_json_bench_corpora.sh` populates the standard external corpus files under `bench/corpus/` so the benchmark suite can run against the common `twitter/github_events/citm_catalog/canada` set
- `scripts/run_json_corpus_bench.sh` runs the recommended corpus-safe benchmark set across the standard corpus file names, keeps low-level `clone_gpa` out of the default comparison set, and only enables object-root indexed modes when the corpus root is an object
