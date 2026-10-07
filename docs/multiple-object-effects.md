# Effects for multiple selected objects

Select several symbols with Shift-click, a marquee or a lasso. The Properties panel shows **Shared effects** for the selected objects, including objects selected alongside raw drawing parts.

The common controls support:

- Alpha and blend mode.
- Tint color and amount.
- Glow color, radius, strength and quality.
- Blur radius and quality.
- Drop-shadow color, blur, offsets and strength.
- Resetting effects for the entire selection.

Values that differ across the selection are marked **mixed**. A dash in an effect checkbox means only some objects have that effect. Clicking that checkbox enables it for the whole selection.

An edit applies only the property you changed. For example, changing tint color keeps each object's existing alpha, other filters, playback settings, position and scale. Enabling an effect keeps the parameters of objects that already have it.

Effects apply to symbol instances rather than their library artwork. On a held or interpolated frame, editing creates keys at the current frame and keeps the original keys intact. The multi-selection and its custom anchor are retained. Locked layers must be unlocked before editing the selection.
