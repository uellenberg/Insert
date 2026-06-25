# Insert

Insert is a programming language for self-modifying code.

![An animation of running the pong game](./pong.avif)

It can be used to create programs that produce modified versions of themselves. For example, the pong game
above ([which was one of the winners of IOCCC29](https://www.ioccc.org/2025/uellenberg/index.html)) can be found
at [test/snapshots/pong-target.stdout](./test/snapshots/pong-target.stdout), with the source code used to create it
at [test/src/pong.int](./test/src/pong.int) and a helper script to run it at [quineRun.sh](./quineRun.sh).

Each iteration of this program produces the source code to create the next frame, with the current frame (display and
game state) rendered inside the source code itself. For a high-level overview of how this all works, check out
the [IOCCC writeup](https://www.ioccc.org/2025/uellenberg/index.html).

## A simple quine

This program prints a copy of itself with an embedded counter incremented each time it runs:

```ts
static incrVal: i32 = binding incrValMarker (0);

static quineStr: &[string] = $quine;

import "std/quine.int";

function main() -> i32 {
    quineStr[incrValMarker] = string(incrVal + 1);
    printQuine(quineStr, $quineLen);
}
```

`$quine` gives you your own source as an array of string fragments, `binding` marks a specific value in that array so
you can find and overwrite it, and `printQuine` prints the result.
See [docs/self-modifying-code.md](./docs/self-modifying-code.md) for how this works.

## Getting started

Make sure you're using rustup, as this project requires a nightly toolchain (specified
in [rust-toolchain.toml](./rust-toolchain.toml)).

```sh
# Compile an Insert program to C.
cargo run -- test/src/pong.int > main.c

# Build and run the C program.
gcc main.c -o a.out && ./a.out

# Or use the included script to iterate it.
./quineRun.sh
```

For a full walkthrough, including how to run a self-modifying program,
see [docs/getting-started.md](./docs/getting-started.md).

## Documentation

- [Getting started](./docs/getting-started.md) - install, build, and run your
  first program.
- [Language guide](./docs/language-guide.md) - types, functions, control flow,
  and the rest of the syntax.
- [Self-modifying code](./docs/self-modifying-code.md) - markers, bindings, and
  quines.
- [Architecture](./docs/architecture.md) - how the compiler is put together.

## High-level architecture

The compiler has three main stages, and you can inspect the output using `--stage`. For a deeper look, including how
quines are emitted, see [docs/architecture.md](./docs/architecture.md).

### AST

First, the input text is transformed into an AST. This step is performed by the `pest` library, and can be found
under [src/parser/program.pest](./src/parser/program.pest).

Then, this AST is converted into MIR (this should really just be IR, although the name is a remnant from an older
version of the compiler that had two levels of IR). This conversion can be found
under [src/parser/mod.rs](./src/parser/mod.rs). This conversion is mostly one-to-one, although a small amount of syntax
sugar is removed.

Here, we declare markers which maintain their order in the program all the way down to the output. We can also declare
bindings, which wrap an expression in two markers (before and after) to modify it specifically.

### MIR

The MIR (mid-level intermediate representation) is where most of the compilation work is done. You can see a high-level
view of what happens in [src/mir/mod.rs](./src/mir/mod.rs) (under `visit_mir`). Initially, the MIR looks like the AST,
and is gradually transformed as more compiler passes run. This transformation includes things like type checking,
constant evaluation, and expression simplification.

After these transformations, the MIR is lowered directly to target code.

### C

C conversion is mostly one-to-one with the MIR. Target lowering works based on tokens (which allow special formatting to
be applied to code), and the C lowering defines special rules for how tokens can be merged (whether spaces are required
to avoid ambiguity). The main lowering code can be found under [src/codegen/c/lower.rs](./src/codegen/c/lower.rs).

To handle quines, the C lowering first creates a full version of the program (separated by markers, and with quine
variables preserved in it). Then, it computes all the quine variables from this and injects them back into the program.
Currently, all this behavior is hardcoded, but in the future it may be possible to write custom code to create different
kind of quines.

Right now, quines work by creating a data array, separated by markers/quine variables, which contains the full content
of the program except for the array itself (which is an empty string). Then, the array is printed out, and when it
encounters an empty string, it prints out the array itself.

There are also some optimizations during lowering to compress the code using defines.