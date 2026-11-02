# q0bo (archived)

q0bo was the editor's mascot. He was a red and black bot with an LED face, and he did three things:
- greeted you on the first start
- played a tutorial in a practice project, drawing and moving real objects with his own pointer
- answered questions in an "ask q0bo" tab

He was taken out of q0editor on 2026-11-01. The code is kept here and is not built.

- `mascot/`: the module as it was in `q0editor/src/mascot/`. It covers his face and moods, speech bubbles, pointer and confetti, the tutorial, and the ask tab with its topics. Tests are included.
- `q0bo.md`: the user guide for him.
- `harness/`: the screenshot harness (`q0bo_shot.rs`, for `out/q0lang-tools/src/bin/`) and its run script, which backs up and restores settings.
- `reapply.py`: the hooks into the rest of the editor, as exact text edits:
  - the `Help` menu entries
  - the `StartTutorial` and `AskQ0bo` actions
  - the `ask q0bo` dock panel
  - the `mascot` settings
  - the places the panels mark for him to point at
  - two small helpers in `app.rs` and `tools.rs`

## Putting him back

```bash
cp -r archive/q0bo/mascot q0editor/src/mascot
python archive/q0bo/reapply.py .
```

`python archive/q0bo/reapply.py . --remove` takes the hooks out again. If the files around them have changed since, the script stops and names the edit that no longer matches.
