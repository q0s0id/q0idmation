# Autosave and crash recovery

q0editor keeps recovery snapshots of unsaved documents so that a crash, a
power cut or a killed process costs at most a couple of minutes of work.

Snapshots are not saves. Your `.q1s`, `.q0l` and `.q0lang` files are written
only when you save. The title bar keeps its `*` after a snapshot.

## What is kept

- Every open project or q0lang source with unsaved changes, including projects
  that were never saved and FLA/XFL imports.
- One snapshot per document, replaced in place. A document that has not changed
  since its last snapshot is not written again.
- Snapshots are written on a background worker through the same verified path
  as a normal save: serialize, parse back, compare, stage, replace.

The timer waits for a quiet frame. It never copies the project during a
stroke, a drag or playback. If a snapshot takes long to write, the next one is
delayed to ten times that duration, so very large projects are not rewritten
continuously.

Saving, discarding or closing a document removes its snapshot. A clean exit
leaves nothing behind.

## After a crash

The next launch shows **Recovered work** at the top of Home, one row per
snapshot, with the document name, its age and its original path.

- **Open** loads the snapshot as an unsaved document under its original path.
  Nothing on disk changes until you save. If the same file is already open
  without changes, that tab is replaced; if it has changes, save or close it
  first.
- **Discard**, then **Delete for good**, removes the snapshot permanently.

A snapshot you opened stays on disk until that document is saved or closed, so
a project that crashes the editor again on opening is still recoverable.

Rows marked **at crash** were written while the editor was already failing.
They hold the newest work but can capture a half-applied edit; the **autosave**
row for the same document is the last snapshot from a quiet frame. Both are
offered when both exist.

Snapshots owned by another running q0editor window are never offered.

## Settings

Settings > Autosave:

- **Keep recovery snapshots**: on by default.
- **Every**: 15 seconds to 60 minutes, default 2 minutes.

The section also shows the age of the last snapshot, or the last error.

## Storage

Snapshots live in `%LOCALAPPDATA%\q0editor\recovery\<session>\`. Each running
editor owns one locked session directory; a directory whose lock is free
belonged to an editor that did not exit cleanly. Emptied session directories
are removed.

Not covered: a hang that never returns to the frame loop gets no new
snapshots, and a panic outside the UI thread writes no at-crash snapshot. The
last timed snapshot remains in both cases.
