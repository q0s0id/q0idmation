# q0editor online

open **Online** at the right of the editor menu. the label shows when you are live in a room, or how many invitations, friend requests and unread messages are waiting.

## signing in

choose **sign in with q0id**. q0id.com opens in your browser; check that it shows the same ten-character code as the editor and choose **yes, connect q0editor**. merely opening the page never connects anything. the editor finishes signing in by itself a few seconds later.

while it waits, the code is selectable (ctrl+c works), and **copy code**, **open browser again** and **copy sign-in link** reach the same request. a countdown shows how long the request stays valid. if the request is declined or expires, the panel says so and offers **try again**; **cancel sign-in** forgets the request so a late approval is never used.

the sign-in lasts while q0editor stays open. sign out from the account menu (**⋯**) in the panel header. q0editor never sees your password.

## rooms

- **rooms** lists invitations first, then **start a room**, then your rooms with your rank, owner, member count and last activity.
- **start room from this document** shares the open project. you become the owner. external project dependencies must be embedded first.
- **join** opens the room in its own document tab once the canvas is idle. your other documents and unsaved work stay as they are.
- **join with a room code** accepts the 32-character code that members can copy from the editor or the website.
- closed rooms stay listed; their owner can reopen them.

room tabs stay free to switch and close. while another document is active, the room keeps its connection and queues project events until you return; those events never replace the other document. **leave** ends live participation and keeps the document open as a local copy; your membership and assigned permissions remain available for a later session. leaving with an edit that hasn't synced yet asks for a second click. closing the editor also ends live participation.

a room is shared, lasting work: nobody leaving, disconnecting or crashing ever closes it, and members can come back to it at any time. only its owner closes it, from **room settings**; a closed room keeps its project and history and can be reopened. **delete** removes a room permanently, including its saved project and history. it lives in room settings next to close, and on the card of an already-closed room; it needs a second click.

## the room tab

- the header shows the connection (**live**, **sending your edit…**, **reconnecting**), your rank, **undo mine**, **redo mine**, **copy code** and **leave**.
- **your access** says what you can edit, whether the layer you are on is editable for you, and lists your rank defaults and personal assignments. **layers you can edit** expands to the full list.
- **people** lists participants with avatars, ranks and where present members are (layer, frame and symbol). **go to** jumps to a participant's timeline and frame once.
- **invite friends** sends invitations. the owner chooses what people join as (editor by default; viewer or admin); everyone else invites viewers. **waiting for an answer** lists pending invitations with **withdraw**.
- the owner opens a participant to change their rank and assignments, and opens **room settings** to choose what each rank can edit (**everything** or **only what you assign**), inviting rules, the member limit, closing and deleting.
- a **room code** lets anyone signed in join as a viewer: paste it in **online → join with a room code**, or on q0id.com/online. the owner can turn this off in the room settings on q0id.com, and then only invitations let people in. banned people stay out either way, and removing someone doesn't stop them using the code again; ban them for that.
- **who may save a copy** (room settings): everyone, editors and up (the default), admins and the owner, or only the owner. anyone below it can watch and draw where they're allowed, but Q0Editor won't save, export or copy layers, frames or artwork out of the room for them, and keeps no recovery copy of it. everyone in a room receives the project to draw it, so this is what Q0Editor promises, not a lock a modified program couldn't pick.
- in an older room where editors or admins can't edit anything yet, the owner sees who is stuck and can let that rank edit everything in one click.

## chat

everyone in a room can talk, viewers included. new lines float over the bottom left of the canvas for a few seconds and fade, like chat in a game, whether or not the Online panel is open. press **/** (change it in **Keyboard shortcuts**: *Chat (online room)*) or click the small **chat** pill to open the input with the recent conversation above it. **Enter** sends, **Esc** closes. the **chat** tab of the panel keeps the full conversation, and q0id.com shows the same chat on the room's page.

send a picture, gif or video with **+** next to the chat or message box (png, jpeg, gif, webp up to 10 MB; mp4 or webm up to 25 MB). pictures show inline; click one to see it full size, videos play in your browser. right-click a line or message to **report** it to the moderators. a line a moderator removed shows as removed.

direct messages show up there too: a line marked **DM** is a message to or from you. in the chat input, `/r text` answers the last person who wrote to you and `/w name text` (or `/msg`, `/dm`) writes to someone directly.

## messages

the **messages** tab is your inbox: conversations newest first with unread counts, **find** to write to anyone by username, and **message** in a friend's **⋯** menu. opening a conversation reads it. in a conversation **Enter** sends and **Shift+Enter** starts a new line. the same conversations live at q0id.com/messages, and the website shows an unread badge in its menu. blocking someone (friends tab) stops messages both ways.

## friends

search by username (at least two characters, matched from the start). answer incoming requests, cancel outgoing ones, and invite friends to the room you are in. remove and block live in each friend's **⋯** menu and need a second click. blocked people are listed at the bottom with **unblock**.

## room permissions

- viewers follow the live project and cannot commit edits.
- editors draw anywhere in a new room. the owner can switch editors to **only what you assign** and give each person layers, timeline folders or library folders.
- admins edit like editors (configured separately) and can also ban lower-ranked participants. an admin who may edit nothing can still moderate.
- the owner assigns ranks and grants, chooses what each rank edits, and closes, reopens or deletes the room.

layers you can't edit show as locked in the timeline and on the stage while the room tab is active, and the stage says why when you try to draw on one. nothing is drawn and then taken back.

in a room, locking a layer and outlining it are yours alone: they change only your view and are never sent, so nobody else's layers lock or turn to outlines. hiding a layer is still shared. a lock that was saved in the document before the room opened unlocks for everyone.

## seeing each other

everyone in the room has a colour, the same on every screen. on the timeline their current frame gets a wash in their colour, and the cell on the layer they are on is filled. their pictures sit at the right of the timeline buttons; hover for where they are, click to jump there. when someone else changes a layer, what changed is outlined on the stage in their colour with their name for a few seconds.

the assignment tree shows your project's own names: each timeline (main timeline first, symbols with their library folder), its layers top-first with timeline folders indented, and library folders by path. ticking a folder covers everything inside it; rows already covered by a ticked folder or by a rank default say so. a library-folder grant gives authority over that folder's symbols and shared artwork, including their existing instances. the server checks every change; the panel only explains.

## undo, conflicts and recovery

undo and redo select only your own operations, and keep everybody else's later work, even strokes added to the same layer after yours. people drawing at the same time are merged: strokes on the same layer and frame, new artwork that happened to get the same id, and reordered layers all combine. only setting the same thing to two different values at once (two people renaming one layer, for example) is a conflict; then the room keeps the version it accepted first and yours is saved to recovery. the room does not open tabs automatically: use **open local copy** or **open unshared copy** when you want one. a change the room refuses (for example a layer you are not assigned) is kept the same way, with the reason named. recovery copies also survive closing the editor.

disconnects retry uncertain edits with the same request id, and the panel shows **reconnecting** until the room answers again. server restarts preserve room checkpoints and the operation journal. if your access ends, the document stays open as a local copy.

global runtime settings and the audio layout currently require a complete-project grant. network work, patch preparation and event reconciliation run outside the editor ui thread, and your edits are sent on their own connection so list refreshes and searches never delay them; stable idle frames do not copy the document. results prepared before a newer local edit are discarded rather than overwriting it. presence is sent when your timeline, layer or frame changes, plus a slow heartbeat.

## website

q0id.com/online shows the same account: invitations, rooms with participants, permissions and the room chat, friends and requests. owners can change ranks, assignments and room settings there too. q0id.com/messages holds your direct messages; profiles have a **message** button.

## implementation and ui handoff

- `q0online/` contains the shared protocol, authoritative permission checks, personal history, and advisory access helpers (`layer_access`, `library_access`, `project_access`, `resource_label`) that interfaces use to explain access.
- `q0online/` also contains the three-way merge (`merge_projects`) the server and editor share.
- `q0editor/src/online/` contains the controller (`mod.rs`), background reconciliation (`sync.rs`), the panel (`ui.rs`), chat and messages with the canvas overlay (`chat.rs`) and the network workers (`transport.rs`).
- `cargo test -p q0editor --lib online::` runs the online tests. `Q0EDITOR_ONLINE_SHOTS=<dir> cargo test -p q0editor --lib online::shots -- --ignored` renders review pictures of the panel without opening a window. an end-to-end test against a running server is described in `transport.rs`.
- `Q0EDITOR_ONLINE_SERVER=http://127.0.0.1:8080` points a development editor at a local q0id server.
- the q0id repository contains `ONLINE_HANDOFF.md` with the complete api, persistence and ui notes, and `tools/editor-online/` with a copy of these desktop sources.
