# Self-modifying code

A quine is a program that prints its own source, and a self-modifying quine is a program that prints a modified version
of its source code. For example, it could print a version of itself with an incremented counter, or one frame of a game
in the future. This page explains the three features that make that possible: markers, bindings, and quines.

Two problems have to be solved for this to work:

1. The program needs access to its own source at runtime. That is `$quine`.
2. The program needs a reliable way to find and replace a specific value in that
   source. That is what markers and bindings do.

If you have not read the [language guide](./language-guide.md) yet, start there.
To learn more about the pong example, see its
[IOCCC29 entry](https://www.ioccc.org/2025/uellenberg/index.html).

Disclaimer: this documentation was written using AI, although I've reviewed and edited it for correctness. It will be
rewritten in the future, but should suffice for now.

## Markers

A marker is a named point in the program that survives all the way to the
output. Markers divide the program into sections, and the compiler guarantees
that the relative order of those sections is preserved through every compilation
pass and into the generated code.

```ts
function main() {
    marker mainInner;
    return;
    marker mainOuter;
}

marker mainEnd;
```

On their own, markers are just anchors. Their real purpose is to mark the
boundaries between the string fragments that make up a quine, so that a fragment
can be located by its marker.

Because markers must keep a stable position, they cannot appear in places where
the compiler might duplicate or drop code. In particular, you cannot put a
marker inside an `inline` or `helper` function, or inside a `const`, since those
get copied to their use sites or removed entirely. The compiler will give you an
error if you try.

## Bindings

A binding wraps a single expression in a pair of markers, one before and one
after. This lets you mark exactly one value so it can be replaced.

```ts
static incrVal: i32 = binding incrValMarker (0);
```

Conceptually, this compiles to something like:

```c
int incrVal = /* incrValMarker */ 0 /* incrValMarkerEnd */;
```

The two comments are the markers that fence off the bound expression. Everything
between them is one fragment in the quine array, and the name you gave the
binding (`incrValMarker`) becomes the index of that fragment. Then, you can write:

```ts
quineStr[incrValMarker] = string(incrVal + 1);
```

The binding name is usable as an integer index into the quine array. Assigning to
that slot replaces the bound expression's text in the copy you are about to
print.

## Quines

The quine variables give your program access to its own source:

| Variable      | Type        | Meaning                                              |
|---------------|-------------|------------------------------------------------------|
| `$quine`      | `&[string]` | The program's source, split into string fragments.   |
| `$quineLen`   | `i32`       | The number of fragments in the quine array.          |
| `$quineSpace` | `char`      | The sentinel character that stands in for a space.   |
| `$quineLine`  | `char`      | The sentinel character that stands in for a newline. |

### How the array is built

During C lowering, the compiler constructs a full copy of the program, split
into fragments at every marker (including the ones bindings insert). That list of
fragments becomes the `$quine` array.

One fragment is special: the entry for the quine array itself is left as an empty
string and is reconstructed at runtime. When the program prints the array, it
walks the fragments in order and prints each one. When it reaches the empty
fragment, it knows that is where the array literal belongs, so it prints the
array's own text there. The result is a complete copy of the source.

### Spaces and newlines

If you want to do any sort of whitespace formatting, the strings in the quine array cannot contain spaces or newlines,
so the compiler replaces spaces and newlines in the fragments with chosen sentinel characters, exposed as `$quineSpace`
and `$quineLine`. When you print a fragment you decode them back.
The standard library's `printQuineItem` does this:

```ts
helper function printQuineItem(s: string) {
    for let i: i32 = 0; s[i] != '\0'; i = i + 1; {
        if s[i] == $quineSpace {
            putchar(' ');
        } else if s[i] == $quineLine {
            putchar('\n');
        } else if s[i] != ' ' && s[i] != '\n' {
            putchar(s[i]);
        }
    }
}
```

You do not normally need to call these helpers yourself. The standard library's `printQuine`
(in [std/quine.int](../std/quine.int)) handles all the functionality needed to output a quine.

## Putting it together

Here is the complete self-incrementing program from
[test/src/quine.int](../test/src/quine.int):

```ts
static incrVal: i32 = binding incrValMarker (0);

static quineStr: &[string] = $quine;

import "std/quine.int";

function main() {
    quineStr[incrValMarker] = string(incrVal + 1);
    printQuine(quineStr, $quineLen);
}
```

Walking through it:

1. `incrVal` holds the counter, and its initial value `0` is wrapped in a binding
   named `incrValMarker`. This is the value that will change between generations.
2. `quineStr` captures the whole program source as an array of fragments.
3. In `main`, `quineStr[incrValMarker] = string(incrVal + 1)` overwrites the
   fragment that holds the counter's value with the next number. The first run
   replaces `0` with `1`.
4. `printQuine` prints the modified source.

Compile and run it once and you get back the same program with `0` replaced by
`1`. Run that and you get `2`, and so on. Each generation is a valid Insert
program that knows its own current count.

## How pong uses this

The pong example ([test/src/pong.int](../test/src/pong.int)) is the same idea at
a larger scale. Every piece of game state - ball position, ball velocity, paddle
positions - is a `static` wrapped in a binding:

```ts
static ballX: i32 = binding ballXMarker (WIDTH / 2 - BALL_WIDTH / 2 - 1);
static ballY: i32 = binding ballYMarker (HEIGHT / 2 - BALL_HEIGHT / 2 - 1);
static ballSpeedX: i32 = binding ballSpeedXMarker (-1);
```

Each frame, the program reads the current state, computes the next frame
(including any reaction to the key the player pressed), writes the new values
back into the corresponding quine fragments, and prints the result. The printed
output is formatted so that, as a side effect of being valid source, it also
draws the current frame on screen.

The driver loop in [quineRun.sh](../quineRun.sh) compiles that output, runs it
with the latest keypress, and feeds its output back in as the next generation.
So the global state variables do double duty: they are ordinary variables while
the frame runs, and they are the values copied forward between generations.

## Limitations and future direction

Quine handling is currently hardcoded in the compiler. The whitespace encoding
with sentinel characters works but is not elegant, and the author has noted a
desire to move that behavior into user code and to support custom quine
strategies via a full compile-time interpreter. For now, the building blocks
above - markers, bindings, and the `$quine` family - are the supported way to
write self-modifying programs. 
