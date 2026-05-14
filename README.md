# Insert

Insert is a programming language for self-modifying code.

![An animation of running the pong game](./pong.avif)

It can be used to create programs that produce modified versions of themselves. For example, see [test/snapshots/pong-target.stdout](./test/snapshots/pong-target.stdout) for a game (see [test/pong.int](./test/pong.int) for the source code and [quineRun.sh](./quineRun.sh) for a helper script to run it).

## High-level Architecture

### AST

First, the input text is transformed into an AST. This step is performed by the `pest` library, and can be found under [src/parser/program.pest](./src/parser/program.pest).

Then, this AST is converted into MIR (mid-level intermediate representation). This conversion can be found under [src/parser/mod.rs](./src/parser/mod.rs). This conversion is mostly one-to-one, although a small amount of syntax sugar is removed.

Here, we declare markers which maintain their order in the program all the way down to the output. We can also declare bindings, which wrap an expression in two markers (before and after) to modify it specifically.

### MIR

The MIR (mid-level intermediate representation) is where most of the compilation work is done. You can see a high-level view of what happens in [src/mir/mod.rs](./src/mir/mod.rs) (under `visit_mir`). Initially, the MIR looks like the AST, and is gradually transformed as more compiler passes run. This transformation includes things like type checking, constant evaluation, and expression simplification.

After these transformations, the MIR is lowered directly to target code.

### C

C conversion is mostly one-to-one with the MIR. Target lowering works based on tokens (which allow special formatting to be applied to code), and the C lowering defines special rules for how tokens can be merged (whether spaces are required to avoid ambiguity). The main lowering code can be found under [src/codegen/c/lower.rs](./src/codegen/c/lower.rs).

To handle quines, the C lowering first creates a full version of the program (separated by markers, and with quine variables preserved in it). Then, it computes all the quine variables from this and injects them back into the program. Currently, all this behavior is hardcoded, but in the future it may be possible to write custom code to create different kind of quines.

Right now, quines work by creating a data array, separated by markers/quine variables, which contains the full content of the program except for the array itself (which is an empty string). Then, the array is printed out, and when it encounters an empty string, it prints out the array itself.

There are also some optimizations during lowering to compress the code using defines.