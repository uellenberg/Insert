# Language guide

This guide covers the ordinary parts of Insert: the parts that look like a
normal language. The features that make Insert special - markers,
bindings, and quines - have their own page in
[self-modifying code](./self-modifying-code.md).

The authoritative description of the syntax is the grammar at
[src/parser/program.pest](../src/parser/program.pest). The examples in
[test/src/](../test/src) exercise every feature and are a good place to look for
working code. You should also review the standard library functions in [std/](../std).

Disclaimer: this documentation was written using AI, although I've reviewed and edited it for correctness. It will be
rewritten in the future, but should suffice for now.

## Comments

```ts
// Line comment.

/* Block
   comment. */
```

## Top-level declarations

A program is a list of top-level declarations: statics, constants, functions,
external functions, imports, target blocks, markers, and raw statements.

### Statics and constants

```ts
const max: u32 = 10;
static sum: u32 = 0;
```

Both bind a name to a typed value. A `const` is evaluated at compile time and
inlined wherever it is used; it never exists as storage in the output. A
`static` becomes a global variable in the generated C, so it can be read and
written at runtime.

Initializers can be full expressions, including calls to functions that the
compiler evaluates at compile time:

```ts
const value1: u32 = complexCalculations(10);
static value: u32 = value1;
```

### Functions

```ts
function add(a: u32, b: u32) -> u32 {
    return a + b;
}

function doNothing() {
    return;
}
```

The return type is written after `->`. If omitted, the function returns `()`
(nothing). Every program that you want to run as an executable needs a `main`
function, which becomes C's `main`.

Functions can be overloaded by argument types. These two are distinct functions:

```ts
function f(value: u32) -> u32 { return 10; }
function f(value: i32) -> i32 { return -10; }
```

#### Function modifiers

A function can be marked `inline` or `helper`:

```ts
inline function string(input: u32) -> string { /* ... */ }
helper function printQuine(strs: &[string], length: i32) { /* ... */ }
```

- `inline` functions are expanded at the call site instead of being emitted as
  real functions. They can take `ref` arguments, which bind directly to a
  caller's variable (more flexible than a `&` reference). Because they are
  duplicated at each call site, inline functions may not contain markers.
- `helper` functions are only emitted if they are actually used, and may be
  dropped otherwise. They also may not contain markers, since they might not be
  included.

### External functions (FFI)

To call into C library functions, declare them with `extern`:

```ts
extern function printf(fmt: string, ...) -> i32 from "<stdio.h>";
```

The `from` clause names the header to include. The compiler tracks which
externals you actually use and emits the smallest set of `#include`s it can. The
`...` marks a variadic function and may only appear at the end of the argument
list. External declarations cannot have modifiers like `inline` or `helper`.

### Imports

```ts
import "std/string.int";
```

Imports pull in the declarations from another file, resolved relative to the
importing file. See [getting started](./getting-started.md#imports-and-the-standard-library).

### Target blocks

A `target` block compiles its contents only for a specific output target. This
is how the standard library keeps FFI declarations C-specific:

```ts
target "C" {
    extern function puts(c: string) -> i32 from "<stdio.h>";
}

target "JS" {
    static x: i32 = 2;
}
```

When you compile to C, only the `"C"` block is included.

## Types

| Type         | Meaning                                       |
|--------------|-----------------------------------------------|
| `i32`        | Signed 32-bit integer.                        |
| `u32`        | Unsigned 32-bit integer.                      |
| `bool`       | Boolean.                                      |
| `char`       | A single character.                           |
| `string`     | A string (a C `char *`).                      |
| `()`         | The unit type, "nothing".                     |
| `&T`         | A reference to a `T` (like a pointer).        |
| `&[T]`       | A slice: a reference to a sequence of `T`.    |
| `[T; N]`     | A fixed-size array reference of `N` elements. |
| `fn(A) -> B` | A function pointer.                           |

These compose freely. A few examples drawn from the test suite:

```ts
&&i32                       // reference to a reference to an i32
&[&i32]                     // slice of references
[i32; 10]                   // fixed array of ten i32
[[i32; 4]; 4]               // a 4x4 matrix
fn(i32, u32, bool) -> string
fn(string, ...) -> i32      // variadic function pointer
&[fn(i32) -> i32]           // slice of function pointers
```

Parentheses can be used for grouping in types, for example `(&i32)` or
`fn((i32)) -> (i32)`.

All arrays are passed by reference, regardless of if they're a slice (`&[T]`) or a fixed array (`[T; N]`). This behavior
exists for C, which doesn't support arrays by value, although if a target that does is added
in the future, this behavior will have to be reworked.

## Variables and assignment

Inside a function, declare locals with `let`:

```ts
let v: u32;          // declared, uninitialized
v = 5;               // assigned later

let g: u32 = 5 * v;  // declared and initialized at once
```

Compound assignment operators are available:

```ts
x += 1;
x -= 1;
x *= 2;
x /= 2;
```

## Expressions and operators

Insert has the usual operator set, with standard precedence:

- Arithmetic: `+`, `-`, `*`, `/`
- Comparison: `==`, `!=`, `<`, `<=`, `>`, `>=`
- Logical: `&&`, `||`
- Ternary: `cond ? a : b`

```ts
static test: u32 = val2 + 5 + 2 * (8 - 5) - 5;
static test2: bool = val1 == val2;
test3 = h ? 1 : 2;
```

Numeric literals can carry a type suffix to disambiguate, such as `10u32` or
`-10i32`.

### References and dereferences

`&` takes a reference, `*` dereferences one:

```ts
function mul(a: &i32, b: i32) {
    *a = *a * b;       // write through the reference
}

function test(val: i32) -> i32 {
    let r: &i32 = &val;
    mul(r, 2);
    return *r;
}
```

### Arrays and indexing

```ts
static a: &[i32] = [1, 2, 3];     // slice literal
static b: [i32; 3] = [1, 2, 3];   // fixed array
static c: i32 = a[1];             // index
static a_ref: &i32 = &a[1];       // reference to an element
static empty: &[i32] = [];        // empty slice
```

You can take a reference into the middle of an array and use it as a smaller
view:

```ts
let view: &[i32] = &a[1];   // a slice starting at index 1
```

### Strings and chars

String and char literals support the usual escapes (`\n`, `\t`, `\\`, `\"`,
`\'`, `\0`, `\r`, `\/`) as well as four-digit unicode escapes like `\u0041` (capital A).
Strings can be indexed to get a `char`:

```ts
static fourth: char = ("test")[2];
```

The standard library's `string()` function converts numbers and booleans to
strings:

```ts
import "std/string.int";

static s: string = string(10u32);   // "10"
```

## Control flow

### if / else

```ts
if h {
    g = 10;
} else if h == false {
    g = 20;
} else {
    g = 30;
}
```

Note there are no parentheses around the condition, and the braces are required.

### loop

`loop` is an unconditional loop, and you can exit it with `break`:

```ts
loop {
    if g == 0 {
        break;
    }
    g = g - 1;
}
```

### while

```ts
while w_i <= max {
    w_acc += w_i;
    w_i = w_i + 1;
}
```

### for

The `for` loop has the familiar three-part header, but note that each part ends
with a semicolon, including the update, and a bare `;` is used for an empty
slot:

```ts
for let i: u32 = 0; i <= max; i = i + 1; {
    sum = sum + i;
}
```

### break and continue

`break` exits the nearest loop and `continue` skips to the next iteration. Both
work in `loop`, `while`, and `for`.

## Raw statements

A `raw` statement injects literal target code (here, C) straight into the
output. It is an escape hatch for when you need something the language does not
express directly:

```ts
raw "/* comment here */\n";

function main() {
    if false {
        raw "return 0;";
    }
    raw "return 1;";
}
```

Use it sparingly, the compiler does not understand what is inside a `raw`
string and will gladly break your code with optimizations.

## Next steps

Once you are comfortable with the basics, read
[self-modifying code](./self-modifying-code.md) to learn about markers,
bindings, and quines, which are the reason Insert exists.
