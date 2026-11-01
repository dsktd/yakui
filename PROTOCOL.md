# yakui protocol (v1, 2026-11-01)

How panes communicate without knowing each other: **one message bus per
space, addressed by topic**. Panes publish and subscribe to topics; nothing
is ever addressed to a pane. Widgets are not on the bus at all. The
vocabulary (topic names, message shapes, event names) lives in one place:
`crates/pane-api/src/protocol.rs` for Rust and `crates/host/web/protocol.js`
for the client; no pane keeps its own copy.

```
widget --emit(event)--> pane web half --send(msg)--> pane Rust half
                                                        |
                                       ctx.bus.publish  |  ctx.bus.subscribe
                                                        v
                                             the space's bus (host)
                                   retained topics persist as data/<space>/bus.json
```

## 1. Topics

A topic is a name and a message shape. A topic is either

- **retained**: the host keeps the last message and delivers it to every
  new subscriber at once, so the topic _is_ shared state (what a late
  joiner, a new pane or a reconnecting client reads); persisted; or
- **transient**: delivered to current subscribers and forgotten; never
  persisted.

Standard topics:

| topic             | kind      | message                                | written by                 |
| ----------------- | --------- | -------------------------------------- | -------------------------- |
| `protocol`        | retained  | `{ "version": 1 }`                     | host                       |
| `scene`           | retained  | `{ "objects": [Object] }`              | host only (via commands)   |
| `selection`       | retained  | `{ "ids": [string] }`                  | host only (via commands)   |
| `hover`           | transient | `{ "id": string \| null }`             | any pane                   |
| `tool`            | transient | `{ "name": string }`                   | any pane                   |
| `focus`           | transient | `{ "id": string }` ("look at this")    | any pane                   |
| `cmd`             | transient | `Command` (below)                      | any pane; consumed by host |
| `rejected/<pane>` | transient | `{ "cmd": Command, "reason": string }` | host, to the sending pane  |

`Object = { id, name, kind: "box"|"sphere"|"light"|..., position: [x,y,z],
size: [x,y,z], color: "#rrggbb", props?: { [key]: JSON } }`. An unknown
`kind` draws as a box; editors keep unknown fields untouched.

Private topics: a pane may use any topic under its own prefix
(`notes/text`, `view3d/camera`), retained or transient as it declares on
first publish, any shape. Other panes may subscribe to them; the shape is
then that pane's documented contract.

Rules: subscribe by name, never by sender. Tolerate a missing or malformed
message (ignore it, never panic, never "repair" a topic you do not own).
Retained messages are whole replacements; there are no patches.

## 2. Commands

Standard retained topics (`scene`, `selection`) are written only by the
host, on **commands** published to `cmd`. The host validates and applies
each command in arrival order, publishes the new retained message, and
publishes `rejected/<pane>` to the sender when a command is invalid. A
command is the result of an interaction, never a stream (a `move_object`
on drop, not per pointer move).

| command          | fields                           | effect                                                  |
| ---------------- | -------------------------------- | ------------------------------------------------------- |
| `select`         | `ids: [string]`                  | `selection.ids = ids` (dedup; unknown ids rejected)     |
| `add_object`     | `object: Object` (`id` optional) | append; host assigns `id` when missing                  |
| `remove_objects` | `ids: [string]`                  | remove; also drop them from `selection`                 |
| `set_property`   | `id, key, value`                 | known key on the object, else `props[key]`; typed check |
| `move_object`    | `id, position: [x,y,z]`          | `position = position`                                   |

`#[serde(tag = "cmd")]` in Rust; `{ "cmd": "select", "ids": [...] }` on the
wire. A rejected command changes nothing.

## 3. Widget events

Widget → pane only, via `emit(name, payload)`; fixed names so a pane can
swap one widget for another without changing its logic:

| event    | payload          | meaning                                    |
| -------- | ---------------- | ------------------------------------------ |
| `select` | `{ ids }`        | the user chose these                       |
| `hover`  | `{ id }`         | pointer over this (`null` = none)          |
| `change` | `{ key, value }` | a field was committed (not each keystroke) |
| `pick`   | `{ value }`      | a one-shot choice (a colour, a tool)       |
| `action` | `{ name }`       | a button or tool was pressed               |

The pane decides what, if anything, to publish for an event.

## 4. Pane-side API

```rust
ctx.bus.subscribe(topic)                    // at new(); retained value arrives first
ctx.bus.publish(topic, value)               // transient or private retained topics
ctx.bus.command(Command)                    // publish on `cmd`
fn on_message(&mut self, topic: &str, msg: Value, out: &mut Out<Self::Event>)
```

`on_message` is a trait method with a default no-op, so a pane that never
subscribes is unchanged. Delivery order per space is arrival order; a slow
pane never blocks the bus (bounded queue; the host logs a drop).

## 5. Versioning

`protocol.version` is retained; additive changes (a new topic, command or
optional field) keep it, a changed meaning bumps it. A pane that does not
know the version shows a message in the pane; it never panics.

## Not in v1

Undo, transactions spanning commands, patches, wildcard subscriptions
(`scene/*`), pane-to-pane addressing (by design, never).
