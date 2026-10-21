# syntax and types

[home](README.md) · [libraries](builtins.md) · [runtime](runtime.md)

## lines and blocks

One statement takes one line. Blank lines are allowed. `#` outside quotes starts a comment that runs to the end of the line. Indentation is for readability; braces define the structure.

An opening `{` ends the header line. A closing `}` stands on a line of its own; a condition may use `} else {`. There are no one-line blocks, no semicolons, no multi-line expressions and no `//` or `/* ... */` comments.

```q0lang
# a comment
text = "inside a string # stays a character"
do! {
  count = 2
  total = count * 3
}
```

`do!` runs its statements right away. It has no scope of its own and no deferred start.

## names and assignment

A name starts with a Latin letter or `_` and continues with Latin letters, digits and `_`. Case matters. Host api names join their parts with dots: `stage.width`, `player.x`. Non-Latin letters are fine inside strings but not in names.

```q0lang
speed = 120
speed = speed + 30
game.score = 0
```

`=` stores a value, `==` compares. There is no separate declaration and no type annotation. `+=`, `-=`, `++` and several assignments on one line are not supported. Do not use keywords as names.

`game.score` is a single name for a shared variable; no `game` object is created. There is no `[]` access.

## values

| type | written as | meaning |
| --- | --- | --- |
| number | `0`, `12`, `0.25`, `-4.5` | a double-precision number; there is no separate integer type |
| string | `"hello"`, `'hello'` | UTF-8 text; both quote styles are equivalent |
| boolean | `true`, `false` | also the result of comparisons and logical operators |
| null | `null` | also what a function returns when it returns nothing |

A number starts with a digit: write `0.5`, not `.5`. There is no exponent notation like `1e3`, no hexadecimal and no digit separators. Minus is a unary operator.

Strings understand `\n`, `\r`, `\t`, `\\`, `\"` and `\'`. For any other escape the backslash is dropped. For paths, `/` or `\\` is easiest. There is no interpolation and no multi-line literal. The old `..."text"` syntax gives a diagnostic.

```q0lang
message = "frame " + 12 + "\ndone"
path = "out/report.txt"
```

An unknown shared name currently evaluates to the internal value `ident` holding the name's text: `copy = missing` stores the symbolic `missing`. It does not become `null`. A typo in arithmetic therefore usually ends in a type error. Rig and target names are best written in quotes. `raw` is an internal value left behind by some errors; it has no literal of its own.

## operators

From highest to lowest precedence:

| level | operators | meaning |
| --- | --- | --- |
| 1 | `(...)`, `name(...)` | grouping and calls |
| 2 | `-value`, `not value`, `!value` | numeric and logical negation |
| 3 | `^` | power, grouping from the right |
| 4 | `*`, `/`, `%` | multiply, divide, remainder |
| 5 | `+`, `-` | add or join strings, subtract |
| 6 | `<`, `<=`, `>`, `>=` | ordering |
| 7 | `==`, `!=` | equality and inequality |
| 8 | `and`, `&&` | logical and |
| 9 | `or`, `\|\|` | logical or |

Apart from power, binary operators of one level group from the left. Unary minus binds tighter than power: `-2 ^ 2` is `4`; for `-4` write `-(2 ^ 2)`. `2 ^ 3 ^ 2` is `512`. There are no bitwise operators.

Arithmetic takes numbers. If either side of `+` is a string, the other side is turned into text and joined: `"2" + 3` gives `"23"`. `2 + true` is a type error. `/` divides with fractions; `%` behaves like the floating-point remainder. Dividing by zero or leaving a function's domain gives infinity or `nan`, not an exception.

Equality never converts types: `1 == "1"` and `1 == true` are `false`; `null == null` is `true`. Ordering compares two numbers or two strings. Strings compare by code point, not by language rules. Write `0 <= x and x <= 1`, not `0 <= x <= 1`.

`false`, `null`, zero, `nan` and the empty string are falsy. Every other number and every non-empty string is truthy, and so are the internal non-empty `ident` and `raw` values.

`and` / `or` return `true` or `false` and evaluate their right side only when they need it:

```q0lang
safe = false and missing_function()
ready = true or missing_function()
within = 0 <= 0.5 and 0.5 <= 1
```

## conditions

```q0lang
progress = 0.75
if progress >= 1 {
  state = "done"
} else {
  state = "in progress"
}
```

`else {` may also go on the line after `}`. Instead of `else if`, put another `if` inside the `else`. Blocks do not create scopes.

## while

```q0lang
count = 0
while count < 4 {
  count = count + 1
}
```

The condition is checked before every pass. An endless loop is stopped by the instruction limit; it does not continue on the next frame. For animation over time, use [frame code](hosts.md).

## for and range

```q0lang
sum = 0
for i in range(1, 6) {
  sum = sum + i
}
countdown = ""
for n in range(3, 0, -1) {
  countdown = countdown + n
}
```

`sum` is `15` and `countdown` is `"321"`.

| header | sequence |
| --- | --- |
| `for i in range(stop) {` | from `0` up to `stop`, step `1` |
| `for i in range(start, stop) {` | from `start` up to `stop`, step `1` |
| `for i in range(start, stop, step) {` | from `start` up to `stop`, step `step` |

`stop` itself is not included. The arguments are evaluated once, left to right, before the loop. Fractions and negative steps are allowed. A step going the wrong way gives an empty sequence. Bounds and step must be finite numbers, and the step may not be zero.

The counter is shared outside a function and local inside one. After the loop it keeps the last value it was given; an empty loop leaves its previous value alone. Changing the counter in the body does not change the sequence. `range` only works in a `for` header; it is not a function that returns an array.

## break and continue

```q0lang
sum = 0
for i in range(10) {
  if i == 2 {
    continue
  }
  if i == 5 {
    break
  }
  sum = sum + i
}
```

`sum` is `8`. `break` leaves the innermost loop. `continue` moves on to the next pass: `for` advances its counter by itself, `while` checks its condition again. Both go on a line of their own and are not allowed outside a loop. A function cannot break or continue the loop of the code that called it.

## functions

```q0lang
pb func distance x y {
  return sqrt(x * x + y * y)
}
pr func announce value {
  q0shell.print! "distance", value
}
result = distance(3, 4)
announce(result)
```

`pb` marks a public function, `pr` a private one. Parameters in the definition are separated by spaces, without parentheses or commas. Calls need the parentheses and commas. The number of arguments must match exactly; there are no defaults and no variable argument lists.

`return value` ends the function at once, nested loops included. `return` without a value, or reaching the end of the body, returns `null`. `return` is not allowed outside a function. A call on a line of its own throws its result away.

The form `pb func handler target` without a body is kept for compatibility: it registers a function that returns `null`. A `do!` after it is a block of its own, not its body.

A definition takes effect once execution reaches it, so define a function before its first call. Defining it again replaces it. Recursion works within the [limits](runtime.md). Do not name your functions `sin`, `clamp` or any other math name: such calls always compile to the built-in.

`pb` / `pr` are kept as metadata; both kinds are called by name in the shared runtime. They are not access control and not module scopes. A nested definition also registers a shared name and does not capture the outer function's locals.

## scope

Top-level variables live on in one runtime between scripts. Parameters, and every plain name the function body assigns anywhere, are local **in the whole body**. Nested `if`, `while`, `for` and `do!` count. Every other name is read from the shared scope.

```q0lang
base = 10
pb func offset amount {
  result = base + amount
  return result
}
answer = offset(5)
```

`answer` is `15`; the locals `amount` and `result` do not become shared. If a function assigns `base`, that `base` is local; reading it before the assignment is an error, not a read of the shared `base`.

Dotted names are never local: a function may write `game.score` or `player.x` and change the shared variable. A plain shared number is easiest to change through a result: `score = add_points(score, 5)`.

Sources: [parser](../../q0s-format/src/q0lang/parser.rs), [compiler](../../q0s-format/src/q0lang/compiler.rs), [qvm](../../q0vm/src/lib.rs).
