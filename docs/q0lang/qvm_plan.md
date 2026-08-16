# qvm implementation plan

qvm is the execution layer for q0lang. source syntax remains owned by q0lang; hosts remain responsible for capabilities.

## data path

`q0lang source -> parser ast -> q0lang compiler -> qvm bytecode -> qvm -> host actions`

## phase 1 - vm foundation — implemented

- [x] standalone `q0vm` workspace crate;
- [x] real bytecode stream and constant pool;
- [x] instruction pointer and operand stack;
- [x] deterministic `QVM\0` bytecode v1 encode/decode;
- [x] source-line table for runtime diagnostics;
- [x] instruction, operand-stack and call-depth budgets;
- [x] explicit host actions: qvm itself has no filesystem/editor/player capability;
- [x] globals persist between q0rg/frame script executions;
- [x] q0lang `Runtime` is now a compatibility facade over compiler + qvm, not an AST walker;
- [x] repeated source is compiled once per runtime and cached as qvm bytecode;
- [x] q0player, q0editor preview and q0shell use the qvm path through the shared runtime;
- [x] regression coverage for bytecode roundtrip, host actions, source-line errors, stack limits and instruction-budget traps.

## phase 2 - language control flow — implemented

- [x] real recursive block AST; no raw `do!` body is reparsed by runtime;
- [x] booleans and `null`;
- [x] `==`, `!=`, `<`, `<=`, `>`, `>=`;
- [x] `and`, `or`, `not` with short-circuit evaluation;
- [x] `if / else`;
- [x] `while`, bounded by the qvm instruction budget;
- [x] real `pb func` / `pr func` bodies;
- [x] arguments, locals, call frames and `return`;
- [x] executable function bytecode persists between separate source executions, so an entry script can define a function used by later frame scripts;
- [x] recursive calls share one instruction budget;
- [x] editor syntax highlighting/completion updated for the real control-flow language;
- [x] editor static script analysis uses a separate small budget so `while true` cannot make the code editor chew the full playback budget every repaint.

## phase 3 - q0 runtime object api — pending

- [ ] stable named instances backed by `instance_id`;
- [ ] scene/q0rg/draw/audio/project host APIs;
- [ ] runtime scene overlay, never mutation of authored `ProjectV2`;
- [ ] mouse/input event dispatch and callable q0lang handlers.

## phase 4 - packaged bytecode — pending

- [ ] formal q0s format revision for compiled qvm chunks, with source/debug metadata optional;
- [ ] compatibility/migration path for source-only older q0s;
- [ ] bundled project resolver compiles linked q0l and embeds child q0s deterministically;
- [ ] q0player prefers verified packaged bytecode and source-compiles only compatible legacy/source content.

## invariants

- qvm has no implicit filesystem/shell capability;
- downloaded q0s cannot acquire q0shell access merely by containing bytecode;
- deterministic execution and bytecode serialization;
- untrusted execution is bounded by instruction/stack/call/transition budgets;
- editor scrub still does not execute irreversible frame side effects;
- q0s-format revisions require compatibility + roundtrip + migration tests.
