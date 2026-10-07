# Fast preview

Use **View > Fast Preview** to play the current unsaved project in q0player.

- **Ctrl+Enter**: all document scenes, in their timeline order.
- **Ctrl+Alt+Enter**: only the current document scene. While editing a nested
  q0rg, this previews its owning document scene.

Both shortcuts can be changed in Settings > Keyboard shortcuts. The existing
Animation Preview panel remains available separately.

q0player opens a separate resizable window containing only the stage. Camera,
vector rendering, effects, animation and audio use the player's normal runtime.
Space pauses/resumes, arrow keys step through frames, Home/End seek, F11 toggles
fullscreen, and Escape closes the preview. Interactive projects keep their game
input bindings. Playback loops the selected scene or the complete scene sequence.

Install an updated q0player, or put its executable next to q0editor.exe. The
editor also checks the standard per-user installation and PATH. An older player
gets an update message instead of being launched with unsupported options.

The editor commits any active text edit and pauses its own playback. It bundles
the current project and its external dependencies into a temporary vector .q0s
on a background worker. This requires no saved project and performs no video
encoding or per-frame bitmap export. Relative dependencies need an owning
project path, as with normal q0s export. Export errors appear in the editor status.

Temporary previews are removed after loading and do not enter q0player's recent
files or save changed player preferences. The authoring document, timeline
position, selection and layout are preserved.

For direct launches:

```text
q0player --preview movie.q0s
q0player --preview --all-scenes movie.q0s
```

`--delete-after-load` is reserved for the editor's disposable previews. Cleanup
only accepts `preview.q0s` inside a `q0e-preview-*` directory directly below the
system temporary directory; ordinary movie files are kept.
