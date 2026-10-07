# Display performance

Settings > Display > Vertical sync (VSync) controls whether q0editor waits for
display refresh when presenting frames. The preference is saved independently
of the project and takes effect after restarting q0editor. Existing installations
keep synchronization enabled by default.

Use the switch to investigate uneven frame presentation when rendering is fast.
The performance snapshot records the active window's mode as
`presentation_vsync=on/off`, even if a different preference is pending restart.
Closing the editor and launching `q0editor --no-vsync` also disables synchronization
for that launch only, without changing preferences. A second launch while the
editor is already open does not change the existing window's presentation mode.

F4 copies a snapshot with several distinct measurements:

- `frame.total`: editor update time.
- `frame.backend-cpu`: the same frame's editor update, tessellation, texture
  uploads and GL submission. Display-refresh waiting and context switching are
  excluded by eframe's timer.
- `frame.interval-including-idle`: wall-clock time between updates, including
  intentional idle waits.
- `frame.active-interval`: intervals following an immediate repaint request
  while the window is focused and visible. These can reveal display/event-loop
  waits, but do not isolate a particular driver or scheduling cause.

The latest 32 update/backend peaks at least 33ms or active intervals at least
50ms remain in the report beyond the rolling 180-frame window. Their age and
repaint causes are included. First-frame initialization costs should be
evaluated separately from later pauses.
