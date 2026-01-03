# Insert

Insert is a programming language for self-modifying code.

## High-level Architecture

### AST

First, the input text is transformed into an AST. This step is performed by the `pest` library, and can be found under [src/parser/program.pest](./src/parser/program.pest).

Then, this AST is converted into MIR (mid-level intermediate representation). This conversion can be found under [src/parser/mod.rs](./src/parser/mod.rs). This conversion is mostly one-to-one, although a small amount of syntax sugar is removed.

### MIR

The MIR (mid-level intermediate representation) is where most of the compilation work is done. You can see a high-level view of what happens in [src/mir/mod.rs](./src/mir/mod.rs) (under `visit_mir`). Initially, the MIR looks like the AST, and is gradually transformed as more compiler passes run. This transformation includes things like type checking, constant evaluation, and expression simplification.

After these transformations, the MIR is lowered directly to target code.

### C

C conversion is mostly one-to-one with the MIR. Target lowering works based on tokens (which allow special formatting to be applied to code), and the C lowering defines special rules for how tokens can be merged (whether spaces are required to avoid ambiguity). The main lowering code can be found under [src/codegen/c/lower.rs](./src/codegen/c/lower.rs).