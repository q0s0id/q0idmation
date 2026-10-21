# rotation tween anchors

Motion interpolation now uses a placement's local `pivot`. The anchor's stage
position follows the authored tween progress; rotation, scale and skew follow
that same progress. Translation is solved from the interpolated anchor and the
current linear transform. Thus a turn around a stationary anchor keeps it
stationary rather than interpolating the object's registration translation
along a chord. Angles remain unwrapped and custom easing/overshoot is retained.

The shared implementation is `q0s-format/src/motion_tween.rs`. The editor's
indexed renderer, software export, native player and timeline snapshots call
it. Local anchors persist through copied/F6 keys and project history; moving
a transform anchor records it on the selected placements. New motion tweens
use the authored point or the artwork's default centre.

q1s v42 and vector q0s v41 add an optional local pivot after each placement's
FX. Documents without pivots keep their previous writer revision. Old revisions
are still readable. For old files, pure turns whose endpoints keep the default
artwork centre fixed recover that missing pivot; translated-and-rotated moves
are not assigned an arbitrary remote fixed point.

Input inspected: F:/Q0S/asdasdsd.q1s, original q1s v32, 60 fps, linear keys at
frames 0 and 85, rotation 0 to 169.40 degrees. The old interpolation displaced
the stationary anchor by about 340 pixels near the middle. Its recovered local
point is (283.72577, 244.18343). Maximum displacement over frames 0..85 is
0.000068239 pixels after correction. The file is read without overwriting it.

The workspace compiles and the editor/player release binaries build. Native
window interaction was not exercised; the supplied document's transforms and
software-rendered middle frame were inspected. No tests were added or run.
Existing test fixture constructors only gained the new optional field.

## numeric rotation

Properties > Rotation (deg) uses the same anchor-preserving rotation helper
as the stage handles. It resolves the saved local pivot, a current custom
selection pivot, or the cached artwork centre, and compensates translation.
A newly resolved pivot is stored only after the edit succeeds, including
materialized hold-frame keys and rig-bound edits. The usual history snapshot
covers the angle, translation and pivot together. Bounds are queried only
when rotation changes, not on every idle Properties repaint.
