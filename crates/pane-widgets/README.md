# pane-widgets

The widget gallery: every widget mounted alone with sample props, and a log of what each
emits. Where it makes sense the emitted value is fed back through `update()`, so the round
trip (props in, intent out, new props in) can be seen without a scene.

## Boundary

- Config: none; Msgs: none; Events: none (a `StatelessPane`: the Rust half is one line).
- Topics subscribed: none. Topics published: none. Widgets are off the bus by design.
- Widgets: toolbar, property-list, number-field, color-swatch, palette-grid, tree.
