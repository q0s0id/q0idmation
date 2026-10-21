# runtime and qvm

[home](README.md) · [syntax](syntax.md) · [host api](hosts.md)

## from source to effect

`source → parser → compiler → qvm bytecode → qvm → host actions → host`

The parser turns source into a syntax tree and reports diagnostics with line numbers. The compiler turns the tree into qvm bytecode. qvm runs the bytecode and collects host actions: jumps, commands, rig changes and so on. qvm itself has no access to files, the editor or the player. The host decides which actions it carries out; see the [availability table](hosts.md).

The shared runtime lives in `q0s-format/src/q0lang/runtime.rs` and qvm in `q0vm/src/lib.rs`. q0editor, q0player and q0shell all run code this way.

## runtime state

One runtime keeps, between the scripts it runs:

- shared variables;
- imported libraries;
- event subscriptions;
- the signatures of public and private functions;
- the compiled bodies of defined functions, so a function from the general script can be called by later frame code.

q0player keeps one shared runtime per movie and one more per placed copy of a symbol with an [object script](hosts.md). Switching scenes keeps the shared runtime. Each runtime caches compiled source: a script that runs on every frame is compiled once.

## values

| value | holds |
| --- | --- |
| number | a 64-bit float; there is no separate integer type |
| string | UTF-8 text |
| bool | `true` or `false` |
| null | nothing |
| ident | the text of a name that was never assigned |
| raw | an internal leftover of some errors |

The rules for operators, truthiness and conversion are in [syntax](syntax.md).

## host actions

| action | made by | host |
| --- | --- | --- |
| `GoRun(target)`, `GoStop(target)` | `gorun!`, `gostop!` | editor preview, player |
| `ShellCommand` | `q0shell.*!` | shell |
| `SceneSwitch` | `q0scene.switch!` | player |
| `RigSetPosition`, `RigSetValue`, `RigReset`, `RigSetPose`, `RigResetPose` | `q0rig.*!` | player |
| `Physics(...)` | `q0physics.*!` | player |
| `Particles(...)` | `q0particles.*!` | player |
| `Light(...)` | `q0light.*!` | player |

A target is either a frame index or a label string. Actions are collected while the script runs; the host applies them in order once it ends. Writes to host names such as `player.x` or `self.x` are read back the same way, after the script.

## diagnostics

Every diagnostic carries the source line it came from. A parse or compile error that makes the program meaningless stops it before it runs. A runtime error, such as a type error, ends that run with a diagnostic; actions collected before it still apply. In q0player, diagnostics are warnings: the movie still opens and plays, and the status line summarises them. q0player keeps at most 256 of them.

## limits

| limit | value |
| --- | --- |
| instructions per run | 100,000 |
| instructions per run in the editor's code analysis | 4,096 |
| operand stack | 4,096 values |
| call depth | 128 nested calls |
| nested expressions | 64 levels |
| nested blocks | 64 levels |
| frame and scene jumps in one chain | 64 |
| active objects with scripts in q0player | 4,096 |

A run that hits the instruction limit stops with a diagnostic; an endless `while` therefore cannot freeze the player. A recursive function shares the budget of the run that called it.

## bytecode

qvm bytecode starts with the magic `QVM\0` and a version (currently 2), followed by a constant pool, the code and a table of source lines. Encoding is deterministic: the same program always gives the same bytes. Version 1 bytecode still loads. Opcodes 41, 42 and 43 carry the physics, particle and light commands and need a qvm that knows them.

## the editor

The q0lang editor window analyses the script being edited with its own small budget. It shows diagnostics, how many statements the script has, the libraries it imports, and a summary of the actions, variables and event subscriptions one run produces. The editor preview runs real frame code; see [host api](hosts.md).

Sources: [runtime](../../q0s-format/src/q0lang/runtime.rs), [compiler](../../q0s-format/src/q0lang/compiler.rs), [qvm](../../q0vm/src/lib.rs), [player](../../q0player/src/lib.rs), [object scripts](../../q0player/src/objects.rs), [editor analysis](../../q0editor/src/q0lang/editor.rs).
