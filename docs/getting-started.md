# Getting started

This page walks you through building the Insert compiler, compiling a program,
and running a self-modifying program.

Disclaimer: this documentation was written using AI, although I've reviewed and edited it for correctness. It will be
rewritten in the future, but should suffice for now.

## Prerequisites

- A nightly Rust toolchain. The exact version is pinned in
  [rust-toolchain.toml](../rust-toolchain.toml), so if you use `rustup` it will
  be picked up automatically. Insert relies on a few nightly features.
- A C compiler such as `gcc` or `clang`. Insert compiles to C, and you build
  the C yourself.
- Node.js, if you want to run the test suite.

## Building the compiler

From the repository root:

```sh
cargo build
```

This produces the compiler binary at `target/debug/Insert`. The test suite uses
the debug build on purpose, because it includes integer overflow checks. You can also
run the compiler directly from cargo with `cargo run -- <args>`, which can be more
convenient.

## Compiling a program

Insert source files use the `.int` extension. The compiler reads one file and
writes C to standard output:

```sh
cargo run -- test/src/simple.int > main.c
gcc main.c -o a.out
./a.out
```

By default the output C is minified (no extra whitespace). If you want to read
it, pass `--fancy` to get indentation and newlines:

```sh
cargo run -- --fancy test/src/simple.int
```

### Command line options

```
Insert [OPTIONS] <INPUT>

  <INPUT>            The Insert file to compile.
  -t, --target      The target language. Only "C" is supported right now
                    (the default).
  -f, --fancy       Format the output with indentation and newlines.
  -s, --stage       How far to take compilation: "parse", "opt", or "target"
                    (the default). See below.
```

The `--stage` option lets you stop early and inspect the intermediate state,
which is useful when you are exploring how the compiler works:

- `parse` prints the MIR right after parsing, before any passes run.
- `opt` prints the MIR after all the optimization and analysis passes.
- `target` (the default) prints the final C.

## Imports and the standard library

Imports are resolved relative to the importing file:

```ts
import "std/string.int";
```

Paths can be relative (`"./helpers.int"`, `"../shared.int"`) or, as with the
standard library, given relative to where you run the compiler. The standard
library lives in [std/](../std) and is itself written in Insert. It provides
things like `string()` for converting numbers to strings, and `printQuine` for
printing a quine.

## Running a self-modifying program

A self-modifying program prints its own next version to standard output. To
actually watch it evolve, you compile and run it in a loop, feeding each
generation's output back in as the next program.

[quineRun.sh](../quineRun.sh) does exactly this for the pong example. The core
of it is:

```sh
while true
do
  gcc main.c -o a.out          # build the current generation
  output=$(./a.out "$key")     # run it, its stdout is the next generation
  echo "$output" > main.c      # overwrite the source with that output
done
```

So the workflow is:

1. Compile your Insert program to `main.c` with the Insert compiler.
2. Build and run `main.c`. Its output is a new, modified `main.c`.
3. Repeat. Each run is a new generation of the program.

For pong this means each generation renders one frame and embeds the updated
game state into the source it prints, so the next generation picks up where the
last left off.

## Running the tests

```sh
node test.mjs            # compile every test and diff against snapshots
node test.mjs --bless    # accept current output as the new snapshots
```

The runner builds the compiler first, then compiles every file in
[test/src/](../test/src) at multiple stages and compares the result against the
saved snapshots in `test/snapshots/`. Files that start with `//@check` must
compile cleanly and files that start with `//@error` must fail with the expected
error message.

## Where to go next

- [Language guide](./language-guide.md) covers the syntax and type system.
- [Self-modifying code](./self-modifying-code.md) explains markers, bindings,
  and quines, which are what make Insert different from an ordinary language.
