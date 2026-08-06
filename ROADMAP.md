# Q0S roadmap

## First beta

The first public test focuses on a dependable editor-to-player workflow:

1. Create and edit a `.q1s` project in q0editor.
2. Save, reopen, and recover cleanly from user-facing errors.
3. Export a validated `.q0s` movie.
4. Open and play the movie in q0player.
5. Install and remove each application independently for the current Windows
   user.

q0editor and q0player are the only applications distributed in this beta.

## After feedback

- Fix reproducible data-loss, file-format, rendering, and installer defects
  before adding new surface area.
- Improve drawing tools and timeline editing from observed beta workflows.
- Expand q0lang only after its runtime and sandbox behavior have dedicated
  compatibility and security tests.
- Package q0shell and q0term only after their filesystem boundary is hardened.
- Document format compatibility before declaring a stable file-format version.

This order is intentionally conservative: saved projects and exported movies
must remain trustworthy while the interface evolves.
