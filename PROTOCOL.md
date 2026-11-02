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
| `owners`          | retained  | `{ "owners": { [id]: pane } }`         | host only (via commands)   |
| `hover`           | transient | `{ "id": string \| null }`             | any pane                   |
| `tool`            | transient | `{ "name": string }`                   | any pane                   |
| `focus`           | transient | `{ "id": string }` ("look at this")    | any pane                   |
| `cmd`             | transient | `Command` (below)                      | any pane; consumed by host |
| `rejected/<pane>` | transient | `{ "cmd": Command, "reason": string }` | host, to the sending pane  |
| `drag/<id>`       | transient | `{ "id", "position": [x,y,z] }`        | the pane holding `id` only |

`Object = { id, name, kind: "box"|"sphere"|"light"|..., position: [x,y,z],
size: [x,y,z], color: "#rrggbb", props?: { [key]: JSON } }`. An unknown
`kind` draws as a box; editors keep unknown fields untouched.

Private topics: a pane may use any topic under its own prefix
(`notes/text`, `view3d/camera`), retained (`retain`) or transient
(`publish`) per message, any shape. Other panes may subscribe to them; the shape is
then that pane's documented contract. A pane may publish only the standard transient
topics and its own prefix; any other write (another pane's prefix, a standard retained
topic, an empty or malformed name) is dropped, and the host answers `rejected/<pane>` with
`{ "topic", "reason" }`. A retained value over 256 KiB as JSON is refused the same way
(a command that would make `scene` or `selection` that big is rejected).

`owners` maps an object id to the pane that holds it (by `grab`, below); it
starts empty whenever the host starts, so nothing stays held across a
restart. `drag/<id>` streams a held object's transform while it is dragged,
at most one message per frame; the host drops (silently, it is a stream) a
`drag/<id>` from any pane but the holder, and paces each `drag/<id>` to at
most 60 messages a second, holding back the latest one (with its original
`ts`) and dropping older held ones. The drop itself is a
`move_object`, so the retained `scene` has the final position; a receiver
draws the stream between its own `scene` updates (`interpolate` in
`protocol.js`) and snaps to `scene` when it changes.

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

| command          | fields                           | effect                                                                |
| ---------------- | -------------------------------- | --------------------------------------------------------------------- |
| `select`         | `ids: [string]`                  | `selection.ids = ids` (dedup; unknown ids rejected)                   |
| `add_object`     | `object: Object` (`id` optional) | append; host assigns `id` when missing                                |
| `remove_objects` | `ids: [string]`                  | remove; also drop them from `selection`                               |
| `set_property`   | `ids: [string], key, value`      | known key on each object, else `props[key]`; typed check, all or none |
| `move_object`    | `id, position: [x,y,z]`          | `position = position` (refused if another pane holds `id`)            |
| `grab`           | `id`                             | `owners[id] = sender`; refused if another pane holds it               |
| `release`        | `id`                             | drop `owners[id]` (refused if another pane holds it; free is a no-op) |

`#[serde(tag = "cmd")]` in Rust; `{ "cmd": "select", "ids": [...] }` on the
wire. A rejected command changes nothing. `set_property` refuses `id` and
`props` as keys (an object's id is fixed; `props` is written one key at a
time). A message on `cmd` that is not a command at all is logged and
dropped: `rejected/<pane>` carries the typed command, so there is nothing to
echo.

`grab` and `release` bracket a drag. Grabs are decided in arrival order:
the first wins, a later one is rejected on `rejected/<pane>` like any other
command (`cmd` is the `grab`, `reason` names the holder), and the loser
drops its local drag and follows the holder's `drag/<id>`. A holder lets go
by `release`, or when its pane is removed, or when the client connection
that last drove the pane closes.

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
ctx.bus.publish(topic, value)               // transient: current subscribers only
ctx.bus.retain(topic, value)                // private retained topic: kept, persisted
ctx.bus.command(Command)                    // publish on `cmd`
fn on_message(&mut self, topic: &str, msg: Message, out: &mut Out<Self::Event>)
```

`Message` is `{ value, origin, seq?, ts? }`: the value, the id of the host it
came from, for a retained message `seq`, a count per topic and origin, so two
hosts' copies of a topic can be ordered later, and for a transient message
`ts`, when its origin published it in milliseconds of that host's monotonic
clock, so a stream of samples can be spaced in time (only differences within
one origin mean anything). `on_message` is a trait method with
a default no-op, so a pane that never subscribes is unchanged. The tolerant
readers in `pane-api::protocol` (`Scene::from_message`, `Protocol::supported`,
`Rejected::reason`, ...) are how a pane reads a standard topic. Delivery order per space is arrival order; a slow
pane never blocks the bus (bounded queue; the host logs a drop).

## 5. Versioning

`protocol.version` is retained; additive changes (a new topic, command or
optional field) keep it, a changed meaning bumps it. A pane that does not
know the version shows a message in the pane; it never panics.

## 6. Shared web modules

The widgets, `protocol.js` and `note.js` are a versioned contract, served at
`/lib/v<protocol.version>/widgets/<name>.js`, `/lib/v<n>/protocol.js` and
`/lib/v<n>/note.js`. The number is `pane_api::protocol::VERSION`, the same
constant `protocol.js` exports as `VERSION`. A pane module imports them by
bare specifier (`import { mount } from 'yakui/widgets/toolbar.js'`); the
shell's import map (`{"imports":{"yakui/":"./lib/v1/"}}`) resolves `yakui/`.
The map is relative, not an absolute `/lib/...`, because the app runs under a
proxy prefix (`/yakui/`) where an absolute path would miss it.

Each widget file opens with `// props:` and `// emits:` lines; a host test
checks it. Changing what a widget's props or emits mean bumps `VERSION`;
adding an optional prop or a new emit does not. A request for another
version is a 404. The old `/widgets/<name>.js`, `/protocol.js` and
`/note.js` redirect (307, relative) to the current version for one release,
so a cached page still works; then they go.

## Not in v1

Undo, transactions spanning commands, patches, wildcard subscriptions
(`scene/*`), pane-to-pane addressing (by design, never).
