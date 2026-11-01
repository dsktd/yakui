# pane-view3d

The `scene` in raw WebGL2: boxes and spheres (any other kind draws as a box, `light` unlit)
with flat shading from screen-space derivatives, an orbit camera (drag to orbit, wheel to
zoom), click to select by reading an id-coloured frame back.

## Boundary

- Config: none (`()`).
- Msgs (web half -> Rust half): `Select { ids }` on a click, `Hover { id }` when the pointer
  comes onto or leaves an object, `SetCamera { camera }` when orbit or zoom comes to rest.
- Events (Rust half -> web half): `State { scene, selection, camera }` (the snapshot),
  `Scene { scene }`, `Selection { selection }`, `Hover { hover }`, `Focus { id }`,
  `Camera { camera }`, `Rejected { reason }`, `Unsupported { version }`.
- Topics subscribed: `scene`, `selection`, `protocol` (retained, standard); `hover`, `focus`,
  `rejected/<pane>` (transient, standard); `view3d/camera` (retained, private).
- Topics published: `cmd` with `select`; `hover`; `view3d/camera` retained as
  `{ yaw, pitch, distance, target: [x, y, z] }` (radians, world units), shared by every view3d
  in the space. All are finite numbers and `distance` is positive; any other `SetCamera` is
  logged and dropped.
- Widgets: toolbar (`reset`, `clear`).
- Smoothness: orbit, zoom and hover picking never leave the browser; one coarse message per
  click, hover change and camera at rest. Objects are not dragged in 3D (view2d does that).
