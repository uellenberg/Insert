# Architecture

This page describes how the Insert compiler is structured, for anyone who wants
to read or change the compiler itself. The compiler is written in Rust and lives
under [src/](../src).

Disclaimer: this documentation was written using AI, although I've reviewed and edited it for correctness. It will be
rewritten in the future, but should suffice for now.

## Overview

Compilation goes through three stages. Each is reachable with the `--stage` flag,
which is handy for inspecting intermediate state.

```
source (.int)
    |
    |  pest grammar + parser        src/parser/
    v
MIR (mid-level IR)
    |
    |  compiler passes              src/mir/
    v
MIR (optimized)
    |
    |  target lowering              src/codegen/
    v
C code (or other targets)
```

The top-level flow is in [src/main.rs](../src/main.rs): parse the input file,
optionally print the MIR (`--stage parse`), run `visit_mir`, optionally print
the MIR again (`--stage opt`), then lower to the target and print it
(`--stage target`, the default).

## Project layout

- `src/` - the compiler, written in Rust.
    - `src/parser/` - grammar ([program.pest](./src/parser/program.pest)) and the
      parser that turns source into MIR.
    - `src/mir/` - the mid-level intermediate representation, where most of the
      compilation work happens.
    - `src/codegen/` - lowering from MIR to a target language.
    - `src/targets/` - target definitions (currently just C).
- `std/` - the standard library, written in Insert.
- `test/src/` - example programs that double as the test suite.
- `test/snapshots/` - expected compiler output for each test.
- `test.mjs` - the snapshot test runner.

## Parsing

The grammar is defined in [src/parser/program.pest](../src/parser/program.pest)
and parsed by the `pest` library. The parser in
[src/parser/mod.rs](../src/parser/mod.rs) walks the resulting parse tree and
builds the MIR.

This conversion is mostly one-to-one with the syntax. It removes a small amount
of syntax sugar, and it records markers and bindings. A binding is desugared into
a pair of markers (a left marker and a right marker) wrapping the bound
expression, which is how a single value gets a stable, addressable position in
the output.

Imports are resolved during parsing. [src/parser/file_cache.rs](../src/parser/file_cache.rs)
avoids re-reading files, and [src/parser/span.rs](../src/parser/span.rs) tracks
source locations for error messages (rendered with the `ariadne` crate).

## MIR

The MIR is the central data structure. It starts out closely mirroring the AST
and is progressively rewritten by a sequence of passes. The high-level driver is
`visit_mir` in [src/mir/mod.rs](../src/mir/mod.rs), and reading that function is the
fastest way to see the full pass pipeline.

The passes and supporting modules include:

- Type checking ([src/mir/type_check.rs](../src/mir/type_check.rs)).
- Constant evaluation via a compile-time interpreter
  ([src/mir/interpreter/](../src/mir/interpreter)). This is what lets a `const`
  be initialized by running real code at compile time.
- Expression handling and simplification
  ([src/mir/expr.rs](../src/mir/expr.rs)).
- Function handling, including inlining and overload resolution
  ([src/mir/function.rs](../src/mir/function.rs)).
- Scope and variable analysis ([src/mir/scope.rs](../src/mir/scope.rs),
  [src/mir/var.rs](../src/mir/var.rs)), including liveness analysis used to reuse
  variable storage once a value is no longer needed (similar in spirit to
  register allocation).
- Lifetime and drop handling ([src/mir/drop.rs](../src/mir/drop.rs)).
- Name mangling ([src/mir/mangle.rs](../src/mir/mangle.rs)), which gives
  overloaded functions distinct target names.
- Marker and binding validation ([src/mir/quine.rs](../src/mir/quine.rs)), which
  rejects markers in positions where code might be duplicated or dropped (inside
  `inline`/`helper` functions and `const`s).
- Optimization passes ([src/mir/opt.rs](../src/mir/opt.rs)).

[src/mir/display.rs](../src/mir/display.rs) implements the human-readable MIR
dump used by `--stage parse` and `--stage opt`.

## Codegen

Lowering turns the optimized MIR into target code. The shared codegen
infrastructure is in [src/codegen/](../src/codegen), and the C backend is in
[src/codegen/c/](../src/codegen/c), with the bulk of the logic in
[src/codegen/c/lower.rs](../src/codegen/c/lower.rs).

Lowering is token-based rather than string-based. Each piece of output is a
token that can carry formatting rules, and the target defines how adjacent tokens
may be merged - in particular, whether a space is required between two tokens to
avoid changing their meaning (see
[src/codegen/c/token.rs](../src/codegen/c/token.rs) and
[src/codegen/token.rs](../src/codegen/token.rs)). This is what allows the default
output to be aggressively minified while staying correct, and `--fancy` to add
readable whitespace.

The C target has its own optimization passes once MIR is converted into a list
of tokens: merging tokens without a space between wherever possible and replacing
repeated tokens with defines ([src/codegen/c/token.rs](../src/codegen/c/token.rs)).

Targets are registered in [src/targets/](../src/targets). Today there is only the
C target, but the structure is built to allow more.

### Emitting quines

Quines are produced during C lowering. The steps are:

1. Build a full copy of the program, split into fragments at every marker (and
   the markers that bindings introduce), with the quine variables preserved in
   place.
2. Compute the quine array from that copy. The fragment for the array literal
   itself is left as an empty string, so the program can detect it at runtime and
   print the array's own text there.
3. Inject the computed array back into the program.

Spaces and newlines inside fragments are replaced with chosen sentinel
characters (exposed to programs as `$quineSpace` and `$quineLine`) so that the
fragments are easy to store and decode. See
[self-modifying code](./self-modifying-code.md) for the language-level view.

This behavior is currently hardcoded. The longer-term plan is to make quine
strategies expressible in user code, backed by the compile-time interpreter.

## Testing

The test harness is [test.mjs](../test.mjs). It builds the compiler, then
compiles every file in [test/src/](../test/src) at several stages (`parse`,
`opt`, `target`, and `target` with `--fancy`) and compares the output to the
snapshots in `test/snapshots/`. A leading `//@check` directive means the program
must compile cleanly and `//@error` means it must fail with the saved error. Run
`node test.mjs` to test and `node test.mjs --bless` to update snapshots after an
intentional change.

```sh
# Run the tests.
node test.mjs

# Update the snapshots after an intentional change.
node test.mjs --bless
```