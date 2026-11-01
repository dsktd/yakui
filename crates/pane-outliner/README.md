# pane-outliner

The `scene` as a tree, grouped by kind. Clicking an object selects it, hovering one hovers
it, double-clicking asks the viewports to look at it; the tree follows the selection and
hover other panes publish.

## Boundary

- Config: none (`()`).
- Msgs (web half -> Rust half): `Select { ids }`, `Hover { id }` (only when it changes),
  `Focus { id }`.
- Events (Rust half -> web half): `State { scene, selection }` (also the snapshot),
  `Hover { hover }`, `Rejected { reason }`, `Unsupported { version }`.
- Topics subscribed: `scene`, `selection`, `protocol` (retained, standard); `hover`,
  `rejected/<pane>` (transient, standard).
- Topics published: `cmd` with `select`; `hover`; `focus`.
- Widgets: tree.
