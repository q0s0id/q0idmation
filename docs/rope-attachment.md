# visible rope attachments

The body artwork and its link must share attachment positions on each rendered
frame. Solver poses alone are not enough: sparse baked object keys interpolate
between poses while links have a drawing on every frame. Live script overrides
and animated scale/skew can also move artwork away from a raw solver endpoint.

`q0physics::attachment_point` maps the original collider attachment into the
artwork's local space, then through the displayed affine transform. The shared
`links::fit_link_lines` deforms each connected run along arc length and fixes
its endpoints exactly. It accepts explicit attachment flags, so a cut or broken
end is never joined back to the body. Rope, chain and spring looks are generated
from the fitted path; the physics rest length and break forces are unchanged.

The editor records per-frame run attachment flags during simulation. Its bake
samples the final body keys through the standard timeline evaluator, including
sparse tweens and authored blank cells, before fitting each link drawing. Active
cells and tween destinations are indexed rather than repeatedly scanning the
whole timeline. The physics preview uses this same bake path. Existing baked
drawings require another bake; exported movies require another export.

The player fits live links against the runtime scene's displayed affine before
generating strokes. Cutting and breakage use the same connected ranges as the
raw physics geometry.

Validation: `cargo check --locked -p q0editor -p q0player -p q0physics` and
`cargo build --release --locked -p q0editor -p q0player` succeeded. No tests were
added or run. The reported visual scenario has not been exercised in a native
window in this environment.
