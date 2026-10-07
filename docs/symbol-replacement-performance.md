# symbol replacement performance

## repeated work removed

The replacement dialog rebuilt and sorted its complete candidate list on every
paint, traversing the symbol graph separately for every candidate. It also
laid out every card in each expanded folder, recalculated visible symbol bounds
and sampled their colors every paint. Video color sampling copied the complete
clip before parsing and decoding its first frame.

- build a reverse symbol graph once per library revision and exclude ancestors
  of the edited symbol; retain the final cycle guard when applying a replacement;
- store the catalog and folder groups in shared immutable arrays, precompute
  lowercase search fields and refilter only when the query changes;
- allocate the full scroll extent but create interaction/paint data only for
  visible card rows; keep the source card's rectangle for initial auto-scroll;
- cache first-frame bounds/background by document render revision and symbol
  frame; bound the cache to 512 entries and invalidate it on asset/live edits;
- progressively prepare unfamiliar visible previews with a 4 ms soft budget
  between cards; a single unusually expensive card can still exceed that budget;
- parse video color samples from shared immutable bytes instead of copying the
  clip; warm card backgrounds no longer require a video decode;
- preserve immutable vector/media caches after replacement. The new render
  revision refreshes dependent frame, placement bounds and effect caches.

The manual frame picker in properties uses the same visible-row grid and
preview metadata cache. Thumbnails still use the native renderer, and apply
existing gradients, masks and effects. Replacement still preserves instance
scale, position, origin and effects; it does not fit artwork into old bounds.

## verification scope

`cargo check --locked -p q0editor --target-dir target` and a release build
completed successfully. No automated tests were added or run. The reported
project and timing capture were not supplied, so the actual native replacement
workflow and its before/after frame times have not been measured.
