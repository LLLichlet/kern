# Kern Source Style

This document records source-style guidance for Kern code in this repository.

It is intentionally short. Language semantics live in
[`docs/design.md`](./design.md); this file only says how to express those
semantics clearly in real code.

## Goals

Prefer source that makes Kern's design visible:

- explicit where the machine-facing boundary matters
- concise where local context is already unambiguous
- expression-driven without hiding control flow
- based on orthogonal language mechanisms, not special-case habits

## Guidance

### 0. Write public docs as Markdown-first API text

Public API docs should be useful when read as Markdown. Use ordinary prose,
headings, lists, code fences, links, and examples directly in `///` or `//!`
comments. Labels such as `Encoding:` or `Compatibility:` are normal prose
unless they are one of the recognized structured doc section names.

Prefer documenting the contract that callers need:

- what the item represents or does
- ownership, lifetime, allocation, and failure behavior
- invariants that are not obvious from the type
- short examples for APIs whose shape is new or easy to misuse

Keep implementation notes in `//` comments near the implementation. A public
doc comment is for the API boundary; an inline comment is for the local
maintenance problem.

### 1. Prefer straight-line unwrapping for simple propagation

When control flow only unwraps a value and immediately exits on failure, keep
the success path straight.

Use `let else` when the shape stays local and the failure arm is still easy to
read:

```kern
let .{ Ok: span } = parse_value_span(text, index)
    else {
        .{ Err: err } => return .{ Err: err },
    };
```

Use `.?` when the surrounding return type already makes the propagation rule
obvious and the operator makes the code shorter and clearer:

```kern
let next = iter.next().?;
let file = open(path).?;
```

When the only extra work is to lift one error type into another, prefer
`map_err(...).?` over spelling the same `Err -> Err` bridge by hand:

```kern
let file = open(path)
    .map_err([](err: fs.Error) Error { return .{ Fs: err }; })
    .?;
```

Use `match` when both branches do substantial work, when you need more than one
success arm, or when the control flow is no longer a simple unwrap-and-return.

Do not force `.?` into places where a visible pattern is clearer than a symbol.

For dispatch over comparable values, prefer `match` over an `if` chain when the
scrutinee and arm values have an `Eq` implementation and the arms describe a
closed local decision:

```kern
match (name) {
    "utf-8" => return .Utf8,
    "utf-16" => return .Utf16,
    _ => return .Unknown,
}
```

Keep `if` when the branch condition is not equality, when each branch needs a
different guard expression, or when ordering is the main behavior being
documented.

### 2. Use `{}` for intentionally empty loop bodies

In scanner-style code, an empty loop body should still use a real Kern block:

```kern
while (index < #text and is_ws(text.[index])) {
    index += 1;
}
```

Reserve this mainly for loops whose whole job is to advance until a condition
fails. Do not generalize the same style to `if (...) {}` by default when a more
direct expression shape would be clearer.

`while (...);` is not valid Kern syntax and should not appear in repository
code or documentation.

Prefer `for` when the code is simply visiting every item from an iterable
source:

```kern
for (byte: text.iter()) {
    ...
}
```

Use `while` when the loop is really driven by mutable state, sentinel parsing,
retry behavior, or an index that advances by more than one ordinary iteration.
That is common in scanners, parsers, terminal editors, and low-level runtime
code. If the condition is subtle, leave a short local `//` comment that explains
the boundary being advanced toward, not a generic note that the loop is a
`while`.

### 3. Let contextual typing do the routine work

Kern has strong source- and context-driven type inference. When the local type
source already fixes the type, omit the type/provider. Repository code,
standard-library code, example packages, and docs should exercise that
inference instead of spelling types out defensively.

Do not write redundant annotations or providers when the type is already fixed
by the local context:

```kern
let mut i = 0;
while (i < #text) {
    ...
    i += 1;
}
```

The same rule applies to BNC, enum variants, associated items, generic
qualification, and literal providers more broadly: if the receiving site
already provides the type source, do not add qualification only because it is
available. Redundant providers make Kern code look half-inferred and weaken the
robustness coverage that the standard library should give the compiler.

Keep explicit providers when:

- width or signedness is part of the logic
- removing the provider would silently change the type to `i32`
- the provider materially improves local readability
- the boundary is ABI-facing, serialization-facing, or otherwise intentionally
  machine-facing

### 4. Be explicit when width is part of the meaning

Kern supports contextual typing, so local literals like `count == 0` are often
good style.

But when bit width is part of the logic, keep the type visible:

```kern
if (byte < 0x20u8) { ... }
```

This especially applies to byte parsers, masks, shifts, pointer-adjacent code,
and other low-level boundaries.

### 5. Omit qualification only when the local context is obvious

Kern may infer enum variants from context:

```kern
return .{ Err: .EmptyInput };
```

That is good style when the expected type is immediate and local.

Keep the type name when the reader would otherwise need to recover the type
from a more distant function signature, parameter list, or trait context:

```kern
parse_literal(text, index, "true", Kind.Bool)
```

The same rule applies to associated items, generic qualification, and literal
providers in general: remove redundancy, but do not make the reader reconstruct
types from far away.

### 6. Use stack mode when repeated pointer-style access is the real shape

Kern does not auto-deref. When a local value is immediately used as a mutable
object for a run of calls, take its address once at the source and keep the
rest of the code pointer-shaped:

```kern
let page = page()..&;
let gpa = gpa().on(page)..&;
let t = test.report(io.stderr())..&;
let state = editor.empty(gpa).should_ok().sum(@loc(), t)..&;

state.handle_key(gpa, .{ Byte: b'i' });
type_text(state, gpa, "hello");
state.handle_key(gpa, .Esc);
```

This is good style when repeated `..&` or `.&` would otherwise dominate the
call sites and the code is semantically operating on one stack-local object.

Do not force stack mode for one or two isolated calls. In short stretches,
`value..&.method(...)` is often clearer.

The same applies to iterator and cursor handles. If a local parser, reader, or
element stream is advanced repeatedly, bind the handle once and call through the
handle:

```kern
let source = reader("<root><leaf/></root>");
let elements = source..&.elements()..&;

let root = elements.next().?.?;
let leaf = elements.next().?.?;
```

This is a readability rule, not a semantic distinction. Use it when the code is
really operating on one stateful object for several steps.

String literals are byte-array value expressions. Use them directly when the
code wants fixed bytes or ordinary array-to-slice decay; bind the value first
when a longer-lived slice or repeated mutation needs a named storage location.

Constructor-shaped helper functions such as `list()`, `map()`, `string()`, or
domain-specific helpers such as `page()` are a package API convention, not a
freestanding language requirement. Prefer them when they express a real
constructor, allocator, builder, or capability boundary. Do not add a helper
only to avoid `T.{}` for a plain aggregate whose fields are the clearest API.

### 7. Prefer explicit module visibility over forwarding boilerplate

Use visibility to describe the intended sharing boundary directly:

- `pub` for package-facing API
- `pub/` for package-internal API
- `pub..` for parent-module-tree API

Prefer these over needless `init.rn` forwarding layers when the real intent is
simply "visible to this package" or "visible inside this parent module tree".

Choose the narrowest visibility that matches the actual boundary. Do not mark
items `pub` by default.

### 8. Prefer mechanisms over privileged library types

Kern source style should lean on general language features such as enums,
patterns, traits, impls, visibility, and explicit control flow.

That means the preferred error-propagation style is built on `let else`,
pattern matching, and `.?` first, not on treating `Option` or `Result` as
privileged language objects.

### 9. Put methods on the weakest useful receiver

Kern impls are for types, and method lookup can use shared-reference methods
from mutable references. If a method only observes a value, put it on `&T`; do
not duplicate the same method in `impl &mut T`.

```kern
impl &String {
    pub fn path() fs.Path {
        return .{ raw: self.as_str() };
    }
}
```

The method above is also available on `&mut String`, because a mutable reference
can be used where a shared receiver is enough.

Use `impl &mut T` only when the method mutates through the receiver, exposes
mutable storage, consumes mutation-only capability, or implements a trait whose
contract is intentionally mutable:

```kern
impl &mut Buffer {
    pub fn clear() void {
        ...
    }
}
```

When both shared and mutable behavior are useful, split them by capability:
query and view methods belong on `&T`; mutation, reservation, deinit, and
mutable-slice access belong on `&mut T`. Only add a more specific receiver such
as `&&mut T` when the distinction is part of the API contract, not as a workaround
for ordinary method lookup.

When a method returns a value that stores a borrow of the receiver, do not put
that method on a value receiver. Use a reference receiver so the API cannot
silently preserve a pointer into a temporary receiver value:

```kern
impl[N: usize] &[N]u8 {
    pub fn reader() io.SliceReader {
        return .{ data: self.*.&[0...N] };
    }
}
```

Use value receivers for lightweight handles and pure value operations whose
result does not borrow from the receiver.

### 10. Prefer fluent capability methods over module-shaped action helpers

When an operation is naturally about one receiver value, make the receiver carry
the public API:

```kern
reader.copy_to(writer);
"build {}".fmt(.{id}).debug();
path.path().write_all_atomic(gpa, bytes);
```

Avoid keeping a parallel public helper that takes the receiver value as an
ordinary argument only because that shape existed first. If shared
implementation is needed, put it behind a private or parent-private helper and
expose one ordinary method-shaped path to users.

Do not expose two public spellings for the same operation just to support both
module style and receiver style. If ordinary usage would read as
`"hello".println()`, do not also publish `println("hello")`; if a pattern is the
real receiver, prefer a pattern handle such as `pattern.regex().compile(gpa)`
and `pattern.regex().find(text, gpa)` over `regex.compile(pattern, gpa)` and
`regex.find(pattern, text, gpa)`. Duplicate spelling makes generated docs,
completion, tutorials, and user code diverge without adding a capability.

Keep module-level functions for constructors, loaders, global state, and
operations without one honest receiver. Once a value exists, let that value be
the action surface:

```kern
let shader = shaders.load_shader("sprite.vs\0", "sprite.fs\0");
defer shader.unload();

let source = "<root><item/></root>";
source.validate(gpa).?;
let index = source.build_index(gpa).?..&;
defer index.deinit(gpa);
```

This keeps public APIs searchable from the domain object a caller already has:
`font.measure(...)`, `path.file_exists()`, `stream.drain_into(sink)`,
`event.render_to(writer)`, and `index.first_child_named(...)` are clearer than a
module full of same-shaped helpers that all repeat the receiver as their first
argument.

Do not preserve old wrapper names just for compatibility before a package has
made a stability promise. A pre-1.0 package should choose the fluent API shape
directly and remove the duplicate public path, so generated docs and completion
show one idiomatic route.

For resources, prefer an owned handle that carries the metadata needed to release
it. Do not expose paired public helpers that require the caller to remember an
original slice, layout, or length just to free the resource:

```kern
let c_path = abi.cstr.owned(gpa, path).?..&;
defer c_path.deinit(gpa);
os.open_file(c_path.ptr(), options);
```

When the resource comes out of a pattern, bind the payload as mutable in the
pattern and then enter stack mode:

```kern
let .{ Some: mut owned_name } = abi.cstr.owned(gpa, name) else return false;
let c_name = owned_name..&;
defer c_name.deinit(gpa);
```

For tests, make assertions postfixed on the checked value and finish them with
`sum(@loc(), report)`. The report value is local, carries the output sink, and
keeps the assertion site explicit without global test state:

```kern
let t = test.report(io.stderr())..&;

"42".parse[i32]().should_ok().eq(42).sum(@loc(), t);
buffer.is_empty().should().sum(@loc(), t);
```

Avoid public test helpers whose only behavior is a silent `@trap()`. A test
failure should report at least its source location and failure kind.

## Tooling Expectations

Run `craft style` during review for source-health metrics. The command is
non-mutating and reports source files, code lines, blank lines,
inline and block comments, doc comments, comment ratios, and public-doc
coverage. Treat these numbers as review signals before they become package
policy.

For mature public packages, the expected direction is:

- public APIs are documented unless the item is intentionally internal to the
  package boundary
- examples and smoke tests cover the primary user-facing workflow
- focused regression tests cover language or library edge cases discovered
  during development
- comment density is high enough to explain invariants and boundaries, but not
  so high that comments restate obvious local code

Low-level runtime code, generated bindings, and experiments may
use different thresholds. Style and policy tools expose severity and scope
controls so packages can choose appropriate review strictness.
Use `[craft.style]` in `Craft.toml` to turn advisory suggestions off, mark them
as warning-level review items, disable specific rules, or exclude generated and
low-level source subtrees from suggestion collection.

`craft fmt` is the deterministic formatting entry point for source-text
normalization. Use it before review, keep higher-level layout consistent with
nearby code, and split long method chains across lines when a postfix chain
stops being quickly scannable.

## Project Authoring Guidance

Packages should make their intended Kern shape visible in the source tree, not
only in external prose. The repository README is the package entry point; module
and item doc comments are the API manual that users see through generated docs,
completion, and editor hovers.

Use the README for:

- package identity, scope, installation, and the first working workflow
- a compact map of public modules and concepts
- links or pointers to examples, benchmarks, and generation steps

Use `//!` and `///` comments for:

- ownership, lifetime, allocation, and cleanup rules
- handle and capability boundaries
- examples that belong next to the API they teach
- domain-specific invariants that callers must remember

Use ordinary `//` comments for local implementation reasoning. Do not leave the
most important usage model only in the README when the code surface can teach it
directly.

### Entry Points And Error Boundaries

Examples should look like real Kern projects. A process `main()` may translate
application failure into an exit code, stderr message, or process-specific
policy, but the main workflow should usually live in an `app()` or `cli()` style
function whose return type carries the useful error:

```kern
enum AppError {
    ReadConfig: xml.ExpectError,
    LoadDocument: xml.IndexError,
}

fn app(gpa: &mut Allocator) void!AppError {
    read_config()
        .map_err([](err: xml.ExpectError) AppError { return .{ ReadConfig: err }; })
        .?;
    load_document(gpa)
        .map_err([](err: xml.IndexError) AppError { return .{ LoadDocument: err }; })
        .?;
    return .{ Ok: {} };
}

fn main() i32 {
    let page = page()..&;
    let gpa = gpa().on(page)..&;
    defer gpa.deinit();

    return app(gpa).success();
}
```

Avoid teaching core library flow as a chain of `else return 1` patterns. That is
only the outer process boundary, and it throws away the domain error that the
package worked to model.

When an example is not fallible, do not invent an error type only to satisfy a
template. A graphical or handle-oriented example can use `main() i32` directly
when the public API itself has no recoverable error to propagate:

```kern
fn main() i32 {
    let win = raylib.window.open(800, 450, "demo\0")
        .target_fps(60);
    defer win.close();

    while (win.is_open()) {
        win.frame()
            .clear(raylib.RAYWHITE)
            .text("hello\0", 20, 20, 24, raylib.DARKGRAY)
            .end();
    }

    return 0;
}
```

### Examples As Compile Contracts

Documentation examples should be close enough to real code that they can be
copied into a package without changing the error or ownership shape. Prefer
full functions when propagation, allocation, or cleanup is part of the lesson.
Short snippets are fine for local receiver methods, but they should still obey
normal Kern rules.

Do not ignore non-void return values accidentally. If a method returns the
receiver for fluent chaining, prefer using the chain:

```kern
let win = raylib.window.open(800, 450, title)
    .target_fps(60);
```

Use `_ = ...;` when discarding the value is the point:

```kern
_ = shader.set_value(location, value, raylib.ShaderUniformDataType.VEC4);
```

For public packages, lock important README and docstring shapes with
compile-only tests. If an example opens a window, touches audio, or otherwise
cannot run in headless CI, keep the calls behind an unreachable branch so the
test still checks names, receiver types, ownership calls, and non-void
discarding:

```kern
if (false) {
    let audio = raylib.audio.open();
    defer audio.close();

    let sound = raylib.audio.load_sound("click.wav\0")..&;
    defer sound.unload();

    sound.set_volume(0.8).play();
}
```

### Public API Shape

A package should expose one idiomatic public route for an operation. Before a
package has made a stability promise, remove duplicate old wrapper names instead
of preserving them as compatibility baggage. Generated docs and completion are
part of the API surface; duplicate helper paths make the package harder to
learn.

For wrapped C libraries or generated ABIs, keep the raw layer package-internal
and build a Kern-facing API by hand:

- generated `raw.rn` or equivalent files should not be the user-facing module
- resource values should have receiver methods such as `texture.draw_at(...)`,
  `image.resize(...)`, `sound.play()`, or `index.first_child_named(...)`
- ownership and cleanup should be visible on the value that owns the resource
- module functions should create, load, or access global capabilities; once a
  value exists, the value should usually be the action surface

Use `[craft.fmt]` and `[craft.style]` excludes for generated source trees whose
layout is owned by a generator:

```toml
[craft.fmt]
exclude = [
    "src/raw.rn",
]

[craft.style]
exclude = [
    "src/raw.rn",
]
```

Do not use generated-source exclusion as an excuse to hide hand-written public
API from formatting or style review. The boundary should be clear: generated
ABI inside, hand-authored Kern API outside.

### Documentation Tone

README text should read like the package itself, not like a placeholder around
an experiment. Avoid describing a package as a "third-party package" from its
own README; state what it provides, what Craft package name users import, and
what lifecycle or ownership rules matter.

Keep tutorial material near the API when it teaches a specific call shape. A
README can introduce the first workflow, but doc comments should carry the
details that users need while editing:

- `//!` for module-level workflows and lifecycle rules
- `///` for item contracts, cleanup, allocation, failure, and small examples
- `//` for implementation notes that should not appear in public docs

Use Markdown normally inside doc comments. Prefer clear prose and code fences
over invented labels or comment conventions that tools do not understand.

## Maturity Gates

Package maturity is a release policy decision, not a language rule. The current
recommended gates are:

- prototype package: `craft check` passes; smoke tests exist for the primary
  path; public-doc coverage is measured but not enforced
- usable public package: `craft fmt --check`, `craft style`, and
  `craft test` pass locally; public-doc coverage is moving upward and missing
  docs are triaged
- mature public package: public-doc coverage is high enough for API review,
  comment ratio is reviewed for invariants and boundaries, smoke tests cover
  the common workflow, and focused regression tests cover discovered edge cases
- performance-sensitive package: optional benchmarks or timing smoke tests
  track the operations that users rely on

Keep the numbers configurable by package role. Generated Vulkan bindings,
runtime internals, freestanding support code, and hand-authored application
libraries should not share one hard-coded threshold. Publish-readiness checks
should report the configured policy and the measured values together so review
can distinguish missing coverage from intentionally relaxed scope.
