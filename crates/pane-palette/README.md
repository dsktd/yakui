# pane-palette

A grid of colours. Picking one sets the colour of every selected object; the swatch of the
first selected object's current colour is lit.

## Boundary

- Config: `{ colors: ["#rrggbb", ...] }`, default twelve.
- Msgs (web half -> Rust half): `Pick { color }`.
- Events (Rust half -> web half): `State { colors, selected, color }` (also the snapshot),
  `Rejected { reason }` (from the host, or this pane's own "Nothing is selected"),
  `Unsupported { version }`.
- Topics subscribed: `scene`, `selection`, `protocol` (retained, standard);
  `rejected/<pane>` (transient, standard).
- Topics published: `cmd` with one `set_property` (`color`) per selected object.
- Widgets: palette-grid.
