# q0online

shared, server-authoritative online foundation for q0editor and q0id. this crate has no ui, database or network runtime dependency. the site persists room state and accepted commands; the desktop adapter prepares patches and consumes accepted events.

## document and protocol

external project dependencies must be embedded before sharing; local filesystem references are rejected.

`protocol_version` and checkpoint `state_version` start at 1. every room has one canonical `projectv2`, a monotonic revision, members, owner-controlled policy, retained events and idempotency receipts. serialized enums use snake_case tags.

`projectpatch::between` splits projects into typed resources: project metadata, project runtime, audio clips, symbol headers, layers with their metadata, assets with their appearances/names/library ownership, and library folders. `projectpatch::apply` compares every `before` value, reconstructs a candidate project and validates it before changing the supplied project. it is the event replay helper, not an authorization entry point. submitted changes must go through `roomstate::submit`.

an operation names the revision it was prepared against. when the room moved on in between, the server rebuilds that revision from retained events and merges (`merge_projects`) instead of rejecting:

- a resource changed on one side takes that side; one changed on both sides merges field by field.
- a layer's placements and explicit keyframes, and the audio arrangement, merge as collections, so people drawing on the same layer at the same time keep all of their strokes.
- assets and layers created with an id someone else took meanwhile are renumbered, with their references.
- document order merges by relative order, not absolute index, so concurrent inserts, deletes and moves combine.
- the same field set to two different values, an edit to something another person deleted, or a merged document that fails validation is a `conflict`. nothing partial is applied.

clients use the same function to carry unsent local work across incoming events, so the editor predicts exactly what the server will accept. the adapter keeps a genuinely conflicting local edit as a recovery copy instead of silently discarding it. `apply_trusted` and `diff` skip whole-document validation for clients; the server validates everything it accepts.

## permissions

- viewers always remain read-only, even when a malformed client claims a project grant.
- editors receive the room's editor defaults plus their explicit member grants.
- admins receive the room's admin defaults plus explicit grants. moderation never implies editing in the engine; editing always comes from defaults or grants.
- new rooms default both editors and admins to the whole project, so people invited to draw can draw. owners narrow a rank to "only what you assign" by clearing its defaults and granting layers, folders or library folders per person.
- only the owner changes ranks, member grants and room policy. ownership cannot be assigned through a normal member update.
- admins can ban or unban lower-ranked participants. they cannot ban the owner, peer admins or themselves. a banned member cannot re-enter through an invitation.
- grants cover the entire project, an exact layer, a layer folder and its descendants, or a library folder and its descendants. folder matching uses path segments, not a raw string prefix.
- moving resources requires source and destination authority. moving a foreign layer into an owned folder cannot claim that layer. mask/folder changes also check affected layers.
- a layer grant permits changing artwork assets only when all canonical referencing layers are editable. a grant to a library folder explicitly authorizes its shared resources; editing such an asset intentionally changes its instances throughout the project. the editor should duplicate shared artwork when a scoped layer edit needs a private copy.
- global runtime settings and audio arrangement currently require a project grant. this keeps camera, physics, scripts and cross-layer references from escaping narrow layer grants.

`layer_access`, `library_access` and `project_access` explain a member's reach for interfaces: whether it comes from ownership, the rank's defaults or a personal assignment. they share the grant matching used by authorization but never authorize anything themselves. `resource_label` names a resource with its project names, and forbidden errors use it instead of internal identifiers.

new invitations admit viewers. editing permissions are a separate owner decision. scopes supplied by clients never authorize an operation: the engine derives affected resources and current folder membership from canonical project data.

deleted identifiers may be reused by normal editor allocators. deleting a layer, symbol or library folder removes grants directly attached to that resource from members and rank defaults. subsequent resource creation receives a new server generation; stale requests prepared before deletion/recreation conflict even when the replacement looks identical. personal undo restores the original generation, and cannot alter a later replacement. grants removed by deletion are not automatically reissued by undo; the owner can explicitly reassign them.

## personal undo

undo selects the caller's latest retained, non-undone edit; redo selects the caller's latest undone edit. undo merges the inverse of that edit with everything that happened since (redo reverses the undo the same way) and checks the caller's current permissions. other users' later work survives, including strokes added to the same layer after yours. if another user later changed the same field, undo reports a conflict instead of restoring a whole-project snapshot over their work. a new personal edit clears only that user's redo branch.

## persistence, reconnect and limits

persist an accepted command before acknowledging it. checkpoint `roomstate` together with its revision and command journal boundary in a transaction. replay commands after that boundary on restart. checkpoints use serde; `validate_loaded` rejects unsupported versions and invalid state.

`events_since` returns the ordered suffix or `history_unavailable`, in which case the client reloads a snapshot. at most 256 events and 16 mib of event data are retained. sha256 request receipts remain after event eviction: an old retry reports `history_unavailable` with its original revision, so it never applies twice. a reused operation id with a different actor, kind or payload conflicts. ids are room-wide ascii identifiers of 1–128 characters.

there are explicit limits: 64 mib serialized projects, 16 mib patches, 4096 changes per operation, 256 members and 128 grants per rank/member. idempotency receipts are kept for every retained event and the newest 65,536 operations; older ones are pruned. clients only retry an operation while it is in flight, and a retry older than retained history resynchronizes instead of applying twice.

presence is temporary, separate from the durable edit journal, and rejects non-finite coordinates. transport authentication, invitation tokens, request rate limits, persistence transactions and network delivery belong to the q0id server.
