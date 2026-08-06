# q0idmation first beta: 10-minute test

Keep the original `.q1s` project while testing. A `.q0s` file is a playback
export, not a replacement for the source project.

Before starting, open `BUILD-INFO.txt` next to the installers and note its
Build ID. Optionally verify both installers against `SHA256SUMS.txt`:

```powershell
Get-FileHash .\q0editor-*-windows-x64-setup.exe -Algorithm SHA256
Get-FileHash .\q0player-*-windows-x64-setup.exe -Algorithm SHA256
```

## Checklist

1. Install q0editor and q0player. Both installers should run without an
   administrator prompt and should use separate install directories.
2. Start q0editor. Create a small drawing with at least two objects on two
   frames.
3. Save it as `.q1s` in a folder whose path contains a space. If convenient,
   also use Cyrillic characters in the folder or file name.
4. Close and reopen the project. Confirm that the objects, frames, colours and
   stage size survived.
5. Make another edit and close the window. Test Cancel once, then save the
   change.
6. Export the project as `.q0s`.
7. Open that `.q0s` in q0player, press Play, seek to another frame and resize
   the window.
8. Double-click both saved files in Explorer. `.q1s` should open q0editor and
   `.q0s` should open q0player.
9. Uninstall one application and confirm that the other still starts.

## Reproducible bug report

```text
Build ID:
Installer filename:
Installer SHA256:
Windows version:
Display scale (for visual bugs):

Steps:
1.
2.
3.

Expected:
Actual:
Exact error text:
Does it reproduce after restarting the app: yes/no
```

q0idmation does not send telemetry and does not upload projects, diagnostics or bug
reports automatically. Only attach a `.q1s` or `.q0s` file if you explicitly
consent and have permission to share everything it contains.
