# Symbol playback ranges

In Properties, a full playback range (first frame 1 and last frame equal to the
symbol's last frame) follows the symbol's length automatically. The last-frame
label shows `auto`. Extending or shortening the symbol updates this range.

Changing either endpoint to a custom value freezes both endpoints. They remain
fixed when the symbol changes length, when the playback mode changes, and after
saving and reopening the project. If the symbol temporarily becomes shorter than
the saved range, playback uses the available frames without discarding the saved
endpoints.

To restore automatic adjustment, set the first frame to 1 and the last frame to
the symbol's current last frame. If the displayed value already matches, confirm
it with Enter.

Existing full-range loops are upgraded when loading older projects. Imported
symbols retain their placement-relative playback timing. The updated editor and
player store this distinction using q1s version 36 / q0s version 35.

## Playback speed

The Speed control beside Playback Mode sets the rate for this symbol instance.
Use 0.5x for half speed, 1x for normal speed, or 2x for double speed. Presets
include 0.25x, 0.5x, 1x, 1.5x, 2x, 3x and 4x; custom values range from 0.01x
to 100x in steps of 0.01x. Different instances can use different rates.

Speed applies to Loop, Play once and their reversed modes. Single frame and
Manual retain the setting but do not use it until automatic playback resumes.
The chosen playback range stays unchanged. Slower playback repeats source
frames; faster playback skips them. Editor preview, player playback and export
resolve the same source frames, including across unchanged keyframes.

Projects with a custom speed use q1s version 37 / q0s version 36. Older files
load at their original 1x rate.
