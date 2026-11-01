# pane-view2d

A front view of the `scene` (x right, y up) on a canvas. Pan by dragging empty space, zoom
with the wheel, drag an object to move it, click to select it, `fit` to see everything.

## Boundary

- Config: none (`()`).
- Msgs (web half -> Rust half): `Select { ids }` on a click, `Hover { id }` when the pointer
  comes onto or leaves an object, `Move { id, position }` when an object is dropped,
  `SetView { view }` when pan or zoom comes to rest.
- Events (Rust half -> web half): `State { scene, selection, view }` (the snapshot),
  `Scene { scene }`, `Selection { selection }`, `Hover { hover }`, `Focus { id }`,
  `View { view }`, `Rejected { reason }`, `Unsupported { version }`.
- Topics subscribed: `scene`, `selection`, `protocol` (retained, standard); `hover`, `focus`,
  `rejected/<pane>` (transient, standard); `view2d/view` (retained, private).
- Topics published: `cmd` with `select` and `move_object`; `hover`; `view2d/view` retained
  as `{ x, y, zoom }` (world units, pixels per unit), shared by every view2d in the space.
  All three are finite numbers and `zoom` is positive; any other `SetView` is logged and dropped.
- Widgets: toolbar (`fit`, `clear`).
- Smoothness: pointer work (pan, zoom, drag, hover hit-testing) never leaves the browser;
  each of the four messages above is one coarse message per interaction.
