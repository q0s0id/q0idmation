# Workspace layouts

The Stage, Tools, Timeline, Library and Properties panels can be rearranged independently of project tabs. Layout changes are shared by all animation projects and do not change the project or its undo history.

## Moving panels

- Drag a panel's tab to move it.
- Drop on the center dock target or another panel's tab bar to group panels as tabs.
- Drop on a directional target to place the panel beside, above or below that panel.
- Drop at the outer edge of the workspace to attach across that side of the workspace.
- Drop away from dock targets to leave the panel floating inside the editor window.
- Drag separators to resize attached panels. Drag floating-window borders or corners to resize both dimensions.
- Right-click a panel tab for **Float panel**, **Hide panel** and **Dock to workspace edge**.

Floating panels stay inside q0editor. They are not separate operating-system windows.

## Saving arrangements

The **Layout** button in the top bar opens the workspace menu:

- **Save layout…** opens the layout manager. Enter a name and choose **Save current layout**.
- Existing presets appear directly in the menu for quick switching.
- **Manage layouts…** lets you apply, update, rename or delete a saved preset.
- **Panels** restores hidden panels. The timeline shortcut also hides or restores the timeline. When the surrounding arrangement is unchanged, a hidden panel returns to its previous dock position.
- **Restore default layout** returns to tools on the left, library and properties on the right, and timeline below.

The last arrangement is saved automatically after dragging stops. Presets include split sizes, tab order, active tabs and floating-window geometry. They are stored with application preferences rather than inside `.q1s` files. Up to 32 named presets can be saved.
