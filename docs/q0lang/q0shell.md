# q0shell: the terminal host for q0lang

`q0shell` is the third program of the q0idmation family, next to q0player and q0editor. It is a plain terminal host: it runs q0lang files without the editor, computes through the shared runtime and applies `q0shell.*!` commands to the file system.

## running it

```powershell
cargo run -p q0shell -- script.q0lang --root .
```

`--root` sets the directory that file operations are allowed in. Without `--root`, the current directory is used.

Short commands need no file:

```powershell
cargo run -p q0shell -- -e "q0shell.print! 'hi'" --root .
cargo run -p q0shell -- -e "q0shell.mkdir! 'out'" -e "q0shell.write! 'out/a.txt', 'Q0'" --root .
```

Each `-e` / `--eval` adds one line of q0lang; several `-e` join into one script. A script path and `-e` cannot be combined. Timeline, rig, scene, physics and particle commands only print that they are unavailable in q0shell.

## commands

```q0lang
q0shell.print! "text", 123
q0shell.read! "path.txt"
q0shell.write! "path.txt", "text"
q0shell.append! "path.txt", "more"
q0shell.mkdir! "dir"
q0shell.list! "dir"
q0shell.move! "from.txt", "to.txt"
q0shell.delete! "path.txt"
```

Every argument is a q0lang expression:

```q0lang
import q0.math
import q0shell

slot = round(sin(pi / 2) * 7)
path = "out/" + "slot-" + slot + ".txt"

q0shell.mkdir! "out"
q0shell.write! path, "armed"
q0shell.read! path
```

`print!` joins its values with spaces. `read!` prints the file, `list!` prints the folder's entries. The longer example in [examples/q0shell.q0lang](examples/q0shell.q0lang) writes, appends, reads, moves and lists files.

## safety

q0shell is deliberately low-level, but not blind:

- absolute paths are refused;
- `..` is refused;
- deleting the root directory is refused;
- every operation resolves inside `--root`.

That covers the first layer of file management: create, write, read, move, delete and list. A movie played in q0player never gets these commands; the player ignores them.
