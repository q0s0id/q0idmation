# graphics folders

Every shape, stroke, fill and text object you draw is stored as a drawing in
the project. Those drawings no longer crowd the library: they sit in
**graphics folders** and only the ones in use are kept.

## where they are

The last row of the library is **Graphics**: a folder with a pen stroke on
it, grey until you point at it, closed until you open it. Inside is one
folder per timeline that has drawings: the Stage (or each scene, by name)
first, then every symbol by name. A drawing used inside a symbol is in that
symbol's folder, not the Stage's, even when the Stage places it too.

They work like any library folder: click the icon or double-click to open or
close, select drawings, drag them onto the stage or delete them.
Searching opens every graphics folder with a match. "Convert to symbol" moves
the drawings into the new symbol's folder by itself, since a drawing's folder
is always the timeline that uses it.

## only what is used

A drawing nothing uses any more (you deleted it, or it was a stray speck or
fill that got erased) drops out of the library. Saving leaves it out of the
file, and the status bar says how many were left out. Until you close the
project it is still there for undo, so undoing the delete brings it back, and
saving after that keeps it again.

"Used" means placed on any layer of any timeline (mask and guide layers
too), chosen by a rig variant or deformer, or drawn by a particle emitter.

## keeping a drawing

A drawing is a plain graphic only while it has no name and no folder of its
own. To keep one in the library even when nothing uses it:

- **name it** (F2 or Rename): it becomes a library item and stays where you
  can see it, or
- **drag it to a folder** of yours.

Dragging it back onto a graphics folder makes it a plain graphic again. Only
drawings in use can go back in: an unused one would vanish as it landed.

## old projects

Nothing about how projects are stored has changed: which folder a drawing is
in is worked out when the library is drawn. Old projects open as they were.
Their named or filed drawings stay where they are. Unnamed drawings at the
library root that are in use move into graphics folders. Unnamed ones that
nothing uses are left out of the file on the next save.
