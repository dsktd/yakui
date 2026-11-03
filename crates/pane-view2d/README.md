# pane-view2d

A front view of the `scene` (x right, y up) on a canvas. Pan by dragging empty space, zoom
with the wheel, drag an object to move it, click to select it, `fit` to see everything.
Another pane's drag shows live and smooth; a drag that loses the grab eases back to it.

## Boundary

- Config: none (`()`).
- Msgs (web half -> Rust half): `Select { ids }` on a click, `Hover { id }` when the pointer
  comes onto or leaves an object, `Grab { id }` when a drag of an object starts,
  `Drag { id, position }` once per pointer move during it, `Move { id, position }` and
  `Release { id }` when it is dropped, `SetView { view }` when pan or zoom comes to rest.
- Events (Rust half -> web half): `State { scene, selection, owners, view }` (the
  snapshot), `Scene { scene }`, `Selection { selection }`, `Owners { owners }`,
  `Drag { topic, id, position, ts, origin }`, `Denied { id, reason }` (a lost grab), `Hover { hover }`,
  `Focus { id }`, `View { view }`, `Rejected { reason }`, `Unsupported { version }`.
- Topics subscribed: `scene`, `selection`, `owners`, `protocol` (retained, standard); `hover`,
  `focus`, `rejected/<pane>`, `drag/<id>` for every object in `scene` (transient, standard);
  `view2d/view` (retained, private).
- Topics published: `cmd` with `select`, `grab`, `move_object` and `release`; `hover`;
  `drag/<id>` as `{ id, position }`; `view2d/view` retained
  as `{ x, y, zoom }` (world units, pixels per unit), shared by every view2d in the space.
  All three are finite numbers and `zoom` is positive; any other `SetView` is logged and dropped.
- Widgets: toolbar (`fit`, `clear`).
- Smoothness: pointer work (pan, zoom, drag, hover hit-testing) never leaves the browser;
  every message but `Drag` is one coarse message per interaction. A drag is predicted
  locally from the first move; a `Denied` drops the prediction. Remote drags draw through
  `interpolate` (one sample behind, at most 100 ms of extrapolation) and snap, eased over
  150 ms, when `scene` changes or the holder lets go.
