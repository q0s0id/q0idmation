# q0lang

q0lang ties code to q0s animation: it computes values, calls functions, and drives frames, named instances, rigs, live physics, particles and lights. One compiler and one virtual machine (qvm) serve q0editor, q0player and the q0shell terminal host.

This reference describes what the current repository implements. What a command does depends on the program running the script: file commands work in q0shell, moving instances and live physics work in q0player.

## quick start

Save this as `hello.q0lang` or `hello.q0l`; both extensions mean the same plain UTF-8 text. The same file is in [examples/hello.q0lang](examples/hello.q0lang).

```q0lang
import q0.math
import q0shell

pb func ease_out t {
  return 1 - (1 - t) ^ 3
}

for i in range(5) {
  progress = i / 4
  x = lerp(0, 100, ease_out(progress))
  q0shell.print! "step", i, "x =", x
}
```

Run it from the repository root:

```powershell
cargo run -p q0shell -- docs/q0lang/examples/hello.q0lang
```

Output:

```text
step 0 x = 0
step 1 x = 57.8125
step 2 x = 87.5
step 3 x = 98.4375
step 4 x = 100
```

`pb func` defines a function, `return` hands back its result, and `for` walks the numbers from zero to four. Calling a function uses parentheses. A command ending in `!` takes its arguments after a space, separated by commas.

## code in an animation

A q0rg's general script suits starting values and functions; it runs once when the timeline starts. Frame code runs every time playback enters that frame. Standalone `.q0l` / `.q0lang` files join through the project graph; for q0player they have to be part of a bundled export. A symbol (q0rg) placed on the stage that has code of its own gets its own runtime per placed copy, see [object scripts](hosts.md).

Put this in a q0rg's general script:

```q0lang
import q0.timeline

pb func start_frame {
  return 12
}

gostop! start_frame()
```

The player jumps to the frame with index `12` and stops; in a shorter animation the index is clamped to the last frame. Frame indices in code start at **zero**, while the editor numbers frames from one. Do not put an unconditional `gostop! 12` into the code of frame `12` itself: the jump runs that frame's code again.

## reference

| page | contents |
| --- | --- |
| [syntax and types](syntax.md) | values, operators, conditions, loops, functions, scope |
| [built-in libraries](builtins.md) | every import, the math functions and all commands |
| [animation and host api](hosts.md) | frames, keyboard, instances, object scripts, rigs, scenes, physics, particles, lights |
| [events](events.md) | mouse subscriptions and how q0player delivers them |
| [q0shell](q0shell.md) | running files, command-line options, file commands |
| [runtime and qvm](runtime.md) | execution order, state, diagnostics, limits, bytecode |
| [recipes](examples.md) | animation math, player movement, physics, particles and lights from code |

Ready-made sources are in [examples](examples). `q0s-format/tests/q0lang_docs.rs` checks that every example in this reference compiles without a diagnostic and that the quick start prints what it shows above.

There are no arrays, objects, classes, closures, exceptions, async functions or user modules through `import` yet. Names like `player.x` are shared host api names, not fields of an object. Named frame labels do not resolve yet.

[qvm_plan.md](qvm_plan.md) and [q0lang_overhaul_plan.md](q0lang_overhaul_plan.md) keep the design history; their plans do not replace this reference.

Sources: [parser](../../q0s-format/src/q0lang/parser.rs), [compiler](../../q0s-format/src/q0lang/compiler.rs), [runtime](../../q0s-format/src/q0lang/runtime.rs), [qvm](../../q0vm/src/lib.rs), [player](../../q0player/src/lib.rs), [object scripts](../../q0player/src/objects.rs), [editor](../../q0editor/src/app.rs), [shell](../../q0shell/src/lib.rs).
