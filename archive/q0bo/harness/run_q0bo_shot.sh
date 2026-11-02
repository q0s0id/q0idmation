#!/bin/bash
# Runs the q0bo harness, then puts back the user's settings and drops any
# recovery snapshots the run left behind.
SCRATCH="/f/Users/q0s0id/AppData/Local/Temp/claude/F--Q0S--CL0UD--q0O0s-Q0S/4dd71561-8778-4b82-998e-a10adf7d60d6/scratchpad"
REC="$LOCALAPPDATA/q0editor/recovery"
before=$(ls "$REC" 2>/dev/null)
cp "$APPDATA/q0editor/settings.json" "$SCRATCH/editor_settings_backup.json"
rm -rf "$SCRATCH/q0bo_shots"
timeout 200 ./target/release/q0bo_shot.exe "$SCRATCH/q0bo_shots" "$@" 2>&1 | tail -30
cp "$SCRATCH/editor_settings_backup.json" "$APPDATA/q0editor/settings.json"
for d in $(ls "$REC" 2>/dev/null); do
  if ! echo "$before" | grep -qx "$d"; then rm -rf "$REC/$d"; echo "removed recovery $d"; fi
done
ls "$SCRATCH/q0bo_shots"
