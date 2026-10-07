# masks and symbol selection

Timeline mask source layers do not contribute to the enclosing q0rg's
selection/transform bounds or recursive body hit testing. The masked content
uses its full original geometry and media bounds, without intersecting them
with the mask. This applies recursively to nested symbols and to both cached
and uncached placement bounds.

Mask source artwork remains editable through its own layer when editing that
symbol. Existing layer visibility/locking and vector erase/fragment geometry
rules remain applicable. The rendering, masking, export and project format
paths are unchanged.

A release q0editor build includes this change. Native UI interaction was not
verified in this turn; no tests were added or run.
