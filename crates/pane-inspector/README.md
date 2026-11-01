# pane-inspector

The first selected object's fields (name, kind, position, size, colour, and whatever is in
`props`), each editable. A committed field becomes one command; the new value shows up when
the host publishes the scene again.

## Boundary

- Config: none (`()`).
- Msgs (web half -> Rust half): `Set { id, key, value }`, one per committed field.
- Events (Rust half -> web half): `Object { object }` (the first selected object or `null`;
  also the snapshot), `Rejected { reason }`, `Unsupported { version }`.
- Topics subscribed: `scene`, `selection`, `protocol` (retained, standard);
  `rejected/<pane>` (transient, standard).
- Topics published: `cmd` with `set_property` (or `move_object` when the key is `position`
  and the value is three numbers). The host checks types; a bad value comes back as
  `Rejected` and the field keeps the scene's value.
- Widgets: property-list (which composes number-field and color-swatch).
- Smoothness: typing stays in the widget; a field is sent only when committed.
