# Timeline video playback performance

Native editor video placements, including video used by the GPU mask compositor,
reuse textures by immutable media source and source-frame index. Both the timeline
and View > Animation Preview predecode up to four upcoming host frames while playing.
Only source frames that those host frames actually display are requested; a 60 fps
video in a 24 fps project does not decode every intervening video frame.

PNG decoding and conversion to egui pixels run on a shared bounded worker pool.
GPU texture creation and submission remain on the UI thread. A first frame, seek,
or unavailable prefetch result still decodes the exact requested frame synchronously;
the renderer never substitutes an older frame or changes authored playback timing.
Paused editor paints do not enqueue new look-ahead work.

Each texture cache reserves at most eight pending/ready images and 64 MiB for
look-ahead pixels. Its existing GPU texture LRU retains at most 64 frames / 128 MiB.
The global worker queue holds at most eight jobs with one to four workers, leaving
one logical CPU available where possible. Removed/replaced sources cancel queued
look-ahead jobs; completed results from another source cannot replace current pixels.

Uniformly transformed vector strokes inside masks and effect groups reuse the same
cached authored stroke triangles as ordinary stage rendering. Nonuniform transforms
retain the existing screen-space fallback to preserve stroke appearance.

Performance snapshots separate these UI-thread costs:

- `video.index`: initial container indexing.
- `video.decode-ui`: synchronous PNG decode on a first frame, seek, or prefetch miss.
- `video.prepare-ui`: synchronous conversion to egui pixels.
- `video.texture-enqueue`: enqueueing an egui texture, excluding later backend upload.
- `mask.software-raster`: whole-scene fallback for masks unsupported by the GPU path.
- `mask.group-strokes`: preparation of group/mask stroke meshes.
- `video.prefetch-hits` / `video.prefetch-misses`: prepared image reuse on texture misses.

Background decode time is not attributed to `App::update`. Backend upload and GL
submission remain in `frame.backend-cpu`. Actual speedup needs a before/after snapshot
from the same project, view and playback range.


## Shared media resources

Video/audio library items keep independent IDs, names, folders and placements.
Identical immutable q0v payloads are shared on import, library transfer and load.
The q1s v38 / q0s v37 writer stores the first payload in full and later media
items as backward references to that payload. Hash matches are always confirmed
by byte equality. References to missing, later or non-media assets are rejected.
Projects without repeated media retain their previous format revision.
Older builds cannot read projects saved with shared-media references; use the
updated editor and player together.

The loader also interns repeated full payloads in legacy files before allocating
their bytes. On Windows, projects up to 4 GiB are read through an immutable file
mapping: the open handle denies writers and deletion until parsing finishes.
Unique embedded q0v payloads have a 1 GiB allocation budget. Reopening and saving
a legacy project compacts repeated media without trimming clips or timelines.
The original file is only replaced after the existing parse-back and atomic
save checks succeed.

Editor video textures, decoder indexes and background prefetch now use source
identity rather than library item ID. The player uses the same source identity
and shared input bytes, and keeps decoded textures within a 128 MiB / 64-frame
LRU budget (textures used by the current paint remain alive). The software
renderer already keys its video cache by the shared source.

These changes remove repeated storage and decode/upload work for identical
media at the same source frame. They do not establish the FPS or compacted file
size of the reported Hank project; that project has not been supplied.
