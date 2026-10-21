# physics time and canvas collisions

gravity and velocity use stage pixels and seconds. a movie frame advances
`time_scale / fps` seconds of the world. comparing the same number of frames
at different movie frame rates compares different durations.

the solver's quality now selects a rate in simulated seconds: the stored
`substeps` value is multiplied by 60, with the default four targeting 240 hz.
each movie frame is split evenly into enough steps to reach that rate. the
default uses the same 1/240-second step at 24, 30, 48, 60, 120 and 240 fps.
other frame rates round the step count up; their steps can be slightly shorter.
the pro physics controls show this as **simulation rate** in hz.

liquid density relaxation previously applied a constant positional pressure
correction on every substep. this gave it an effective acceleration proportional
to the squared step rate, overpowering gravity as fps increased. pressure now
scales with dt squared, and surface friction uses elapsed time too. the previous
24 fps / four-step default is the coefficient reference, preserving its tuning.
editor preview, bake and live q0player share these calculations.

**physics → world → collide with canvas edges** switches all canvas collisions
on or off in both simple and pro modes. individual sides remain configurable.
turning the switch back on restores the last enabled sides for that project and
timeline during the editor session. a reopened world with no active sides starts
with all four when enabled. the enabled sides, including none, are saved in the
existing wall mask and used by bodies, ropes and liquids in preview, bake and
live playback. undo restores the same project setting.

already baked animation is ordinary keyframes. rebake it to use updated physics
or changed canvas collisions.
