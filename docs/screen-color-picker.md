# screen color sampling

Open any color swatch in Properties (fill, stroke, background, gradient stop,
tint/glow/shadow), text controls, or theme settings. Inside the palette, the
existing color square, hue, alpha and numeric controls remain editable.

With the palette still open, click anywhere outside it to pick the displayed
screen pixel and close the palette. On Windows this includes editor UI, other
windows and other monitors. The click belongs to color sampling: it does not
select/move artwork or activate the clicked button/application. Esc closes the
palette without sampling. Sampling preserves the destination color's alpha.

Windows reads one desktop pixel with GetPixel on mouse press and applies it on
release. A temporary low-level mouse hook consumes that press/release pair. It
is removed when the popup closes, its field disappears, or its document/symbol
context changes. There is no continuous screen capture or screenshot encoding.
The popup's physical bounds come from egui/winit's viewport coordinates and
pixels-per-point, including UI zoom and monitor scale. Other platforms (or a
failed native hook) fall back to one-shot editor framebuffer sampling.

The shared widget reports sampled edits through Response::changed, preserving
the existing callers' edit and history paths. The active drawing tool and stage
selection remain in place; inline text is not committed by a sampling click.

Validation: the Windows editor compiles. Native desktop interaction has not
been manually exercised in this environment.
