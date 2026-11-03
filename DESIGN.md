# yakui: design (draft 2, 2026-10-31)

## Goal

A UI whose top level is only Blender-style panes and splits. Each pane is
independent and knows nothing outside itself. Any single pane, or the whole
layout, can be viewed remotely: phone browser, laptop browser, or the Tauri
window on the host.

**Top priority (Cal): a smooth experience.** Every other choice below yields
to that. In practice it means: no interaction ever waits on a socket round
trip, 60 fps for drag/resize/scroll, instant pane switching, and the remote
view feeling live rather than polled.

Non-goals for v1: theming system, plugin marketplace, multi-user editing.

## Decisions (Cal, 2026-10-31)

- Name: **yakui**.
- Substrate: Tauri shell with web panes.
- Isolation: in-process Rust trait, compiler-checked.
- Remote view: required (mobile web, laptop).
- State per client by default, shared on opt-in (2026-10-31, replaces the
  "one shared layout" of draft 2; see "Spaces").
- Panes: hybrid (Rust half for durable state, web half for interaction;
  see "Pane authoring").

## Spaces: per-client by default, shared on opt-in

Cal, 2026-10-31: several clients are not a feature of the app, they are how
a web app is reached. Each client is its own app, but sharing stays
available as an opt-in.

A **space** is one layout tree plus its pane instances (and their storage,
`data/<space>/`). The host keeps a map of spaces. Every client belongs to
exactly one space; a space may have any number of clients. Today's host code
(fan-out, snapshot on join, correction frames) is the many-clients-per-space
case and stays as is; the one-client case is the same code with one
subscriber.

- Identity: the shell generates a random id once and keeps it in
  `localStorage`; that is its default space. Another browser, another space.
  A connection is not an identity: a reload or reconnect returns to the same
  space.
- Opt-in sharing: the space id is in the URL, `/s/<space>/` and
  `/s/<space>/pane/<id>` (pane ids are per space). Opening another client's
  link joins that space; a "share" action in the shell shows the link. The
  shell at `/` goes to the client's own space (the id lives in the browser,
  so the host cannot redirect); the host makes a space on its first
  connection.
- Read-only stays a per-connection flag (`?ro=1`), so a shared link can be a
  passive mirror.
- The Tauri window is a webview with its own storage, so a separate client
  (and space) from the desktop browser unless given a link.
- Later, if wanted: a page listing spaces, renaming, deleting an empty one.
  Not v1.

## Reconsidered for smoothness

Draft 1 routed every pane interaction through `Pane::update` over the
WebSocket. On localhost that is sub-millisecond; over LAN/VPN it is the
network RTT on every keystroke and drag, which is exactly not smooth. Changes:

1. **Two halves per pane, with a rule.** Each pane has a Rust half (durable
   and shared state, the thing `snapshot()` returns) and a web half (ES module
   that renders and handles interaction). Rule: anything that must survive a
   reload or be seen on another device goes through Rust; anything that is
   pointer- or keystroke-level interaction stays in the client and is sent to
   Rust as a coarser message (`SetText`, not `KeyPress`). The web half may
   apply its own message optimistically and reconcile on the next event.
2. **Layout operations are client-first.** Split/resize/join apply in the
   client immediately and are confirmed by the host; the host is the source of
   truth and broadcasts the result to other clients. Resizing by drag sends a
   final ratio on release, not a stream of ratios.
3. **Snapshots are small and events are deltas.** `snapshot()` is for late
   joiners; steady-state traffic is `Event` deltas. A pane whose snapshot is
   big (notes, logs) must page or truncate.
4. **Webview risk, Linux.** Tauri uses the system webview (WebKitGTK on
   Linux), which is the weakest part of the smoothness story for animations
   and heavy DOM. Mitigation is built in: Tauri is a window around a URL, so a
   Chromium `--app=` window or any browser is a drop-in shell for the same
   client. Measure early (milestone 1 includes a drag-resize frame-time check
   in both shells) and pick the default shell on evidence.
5. **Web-side isolation.** Rust enforces isolation of state; the DOM still
   needs a fence. Each pane's web half mounts into a shadow root, gets its
   rect as CSS containment (`contain: strict`), and is given no globals beyond
   a `send(msg)`/`onEvent(cb)` pair. An iframe per pane would be a harder
   fence but costs layout smoothness and complicates input; keep it as the
   escalation if shadow roots prove leaky.

## Architecture

Three layers; only the middle one knows about layout.

1. **Pane crates** (`pane-*`). Each implements one trait, depends only on
   `pane-api`, never on another pane or on the host. Owns its state and its
   own web half (one self-contained ES module + CSS, no shared globals).
2. **Host** (`host`, Rust, axum). Owns the layout tree, the pane registry and
   the transport. Creates panes by type name, routes messages by pane id.
3. **Shells**. Tauri window and plain browser both load the same web client
   from the host over HTTP + WebSocket. Tauri is just a window around
   `http://127.0.0.1:<port>`; remote viewing is the same URL on the LAN
   behind auth. No second code path.

### Pane trait (`pane-api`)

```rust
pub trait Pane: Send + 'static {
    /// Stable type name, used by the registry and layout files.
    const KIND: &'static str;
    type Config: Serialize + DeserializeOwned + Default;
    type Msg: Serialize + DeserializeOwned;   // client -> pane, coarse
    type Event: Serialize;                    // pane -> client, deltas

    fn new(config: Self::Config, ctx: Ctx) -> Self;
    fn update(&mut self, msg: Self::Msg, out: &mut Out<Self::Event>);
    fn snapshot(&self) -> Self::Event;        // full state for late joiners
}
```

`Ctx` is deliberately tiny: pane id, an async timer/spawn handle, a logger and
a per-pane storage directory. No handle to the layout, the host or other
panes. If a pane needs data from outside, that is a service the pane is
configured with (a URL in `Config`), not a reference to another pane.

Isolation is enforced by crate dependencies: a `pane-*` crate that imports
another pane or `host` fails review and a CI `cargo deny`/`cargo tree` check.

### Pane authoring: Rust-only vs. pure-web (decided: hybrid)

|                           | Rust-only (web half is a thin renderer)                                                     | Pure-web allowed (no Rust half)                                         | Hybrid (Rust half required, may be trivial)                     |
| ------------------------- | ------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------- | --------------------------------------------------------------- |
| Smoothness                | Worst by default: every interaction is a message unless the author adds client state anyway | Best for local interaction: everything in-browser                       | Same as pure-web for interaction; durable state via Rust        |
| Remote/phone view         | Consistent for free: snapshot is truth                                                      | Broken by default: state lives in one browser; needs per-pane sync code | Consistent for free                                             |
| Isolation                 | Compiler-checked                                                                            | Convention only (shared JS runtime)                                     | Compiler-checked for state; DOM fence as above                  |
| Prototyping a visual pane | Slowest (two files, types, serde)                                                           | Fastest                                                                 | One trivial Rust impl (`Config = ()`, no state) + the ES module |
| Code paths                | One                                                                                         | Two                                                                     | One                                                             |

Recommendation: **hybrid**. It is pure-web's smoothness with Rust-only's
remote consistency and isolation, at the cost of a few boilerplate lines for
panes with no state. Make that boilerplate a `pane-api` helper
(`StatelessPane<"kind">`) so a pure-visual pane is one Rust line.

### Layout (host)

A binary tree: `Split { axis, ratio, a, b }` or `Leaf { pane_id }`. Operations
are the Blender set: split, join, swap, resize, change pane type. The layout is
data (serde, saved as a file); panes never see it. Pane type change = drop the
pane, create another, same leaf. Clients apply layout ops optimistically (see
above); the host confirms or corrects.

### Transport

One WebSocket per client. Frames are `{pane_id, payload}` JSON for v1
(switch to a binary encoding only if measured). The host forwards client
messages to `Pane::update` and fans events out to every client subscribed to
that pane. A new client gets the layout and each visible pane's `snapshot()`.
Reconnect is automatic with a fresh snapshot; the client shows a thin
"reconnecting" strip rather than freezing.

URLs: `/s/<space>/` full layout, `/s/<space>/pane/<id>` one pane filling the
viewport (the mobile case), `/ws?space=<space>`; `/` is the shell too, which
goes to the browser's own space. The web client is one small shell that
mounts each pane's ES module into its rect.

### Remote access and auth

Bind 127.0.0.1 by default. LAN or remote exposure only behind an
authenticating proxy (same rule as the Caddy proxy here). Read-only vs
read-write is a per-connection flag so a phone can view without the ability to
send messages.

## Widgets and the document (2026-11-01)

Cal: build a gallery of example panes and panel widgets; content does not
matter, each pane and each widget must have a clean, well-designed API
boundary. Two additions make that possible.

### Widgets

A widget is a reusable UI piece inside a pane's web half: a property list, a
colour palette, a tree, a toolbar, a number field. It knows nothing about
panes, spaces, sockets or the document. Contract, one ES module per widget
under `widgets/<name>.js`, no shared globals:

```js
export function mount(el, props, emit) -> { update(props), destroy() }
```

- `el` is an element the widget owns completely (it may use a shadow root).
- `props` is plain data; the widget renders it and nothing else. The same
  props must render the same widget (no hidden state beyond transient UI
  state such as hover or an open dropdown).
- `emit(name, payload)` is its only way out: a widget reports intent
  ("change", "select", "pick"), never performs it. The pane decides what to
  do, usually a `send(msg)` to its Rust half.
- `update(props)` re-renders with new props; `destroy()` removes listeners.

Panes compose widgets; widgets never compose panes and never import other
panes. A widget may import another widget. The host does not know widgets
exist: they are served as static files from the host's versioned
`/lib/v<n>/widgets/` (see "Shared web modules" in `PROTOCOL.md`), imported
as `yakui/widgets/<name>.js` through the shell's import map.

### The bus

Panes share nothing but a per-space **message bus** addressed by topic:
`PROTOCOL.md` is the spec (topics, retained vs. transient, commands, widget
event names). Retained topics are the shared state a late joiner reads;
transient ones are events. Standard state topics are written only by the
host on commands, so one validator holds the schema. No pane is ever
addressed; nothing references another pane. Cal, 2026-11-01: "no direct
pane-to-pane communication but rather a general message or event bus."

### Gallery (first set)

Each pane: a Rust half (state it owns, topics it subscribes to and publishes,
messages/events) and a web half built from widgets. Each with a short
README section in its crate stating its boundary: config, msgs, events,
topics subscribed and published, widgets used.

- `pane-view2d`: canvas, pan/zoom, draws the `scene` objects as shapes,
  click selects (`select` command). Widgets: toolbar.
- `pane-view3d`: WebGL, orbit camera, same `scene`, click selects. Widgets:
  toolbar.
- `pane-inspector`: shows the selected object's properties, edits write back
  into `scene`. Widgets: property list, number field, colour swatch.
- `pane-palette`: colour palette; picking a colour sets the selected
  object's colour. Widgets: palette grid.
- `pane-outliner`: tree of `scene` objects, selection both ways. Widgets:
  tree.
- `pane-widgets`: the widget gallery itself: every widget with sample props
  and an event log, so a widget can be seen alone.

Acceptance for the boundary: `cargo tree` still shows only `pane-api` from
the workspace for every pane; no pane imports another pane's module in JS;
every widget module has no imports except other widgets; the topics each
pane depends on are written down in its README section.

Built 2026-11-01, as PROTOCOL.md says, with these choices:

- The scene schema is PROTOCOL.md's: `scene = { objects: [Object] }`,
  `Object = { id, name, kind, position: [x,y,z], size: [x,y,z], color: "#rrggbb", props? }`;
  view2d shows the x/y plane (y up), view3d the same scene with an orbit
  camera. Panes read it with the tolerant readers in `pane-api::protocol`
  (a bad object is skipped, a bad topic reads as empty).
- Seeding: the host retains four objects (fixed ids) as `scene` when a
  space's bus has none, before any pane subscribes; a viewport never seeds
  and tolerates a missing `scene`.
- Private retained topics: `view2d/view` (`{ x, y, zoom }`) and
  `view3d/camera` (`{ yaw, pitch, distance, target }`), one per space, so a
  reload or another client sees the same view.
- `pane-api::Bus` is `subscribe`, `publish` (transient), `retain` (private
  retained) and `command`; the trait method is `on_message(topic, Message)`,
  where `Message` is the value plus its `origin` host and, if retained, its
  `seq`. The host's bus is a trait (`host::bus::Bus`) with one in-process
  implementation, and pane ids are `<host>-<n>`, so another transport can
  come later without touching panes.

## Distributed: Iroh (decided 2026-11-01, after the gallery)

Cal: run yakui across several PCs, each hosting its own panes, all on one
bus. Iroh over libp2p: Rust-native, QUIC by node id with relays and hole
punching, and it ships both halves of the bus split.

- **Transient topics** ride `iroh-gossip`, one gossip topic per
  `<space>/<topic>`. Fire and forget, as now.
- **Retained topics** ride `iroh-docs`: an eventually consistent key-value
  store with set-reconciliation sync, so a joining host receives the
  current `scene` and `selection`. Standard retained topics keep one
  **authority** (the host that owns the topic applies commands, stamps
  `seq`, writes the doc entry); other hosts forward `cmd` to it. Private
  retained topics are last-write-wins per key as iroh-docs does.
- **Remote panes**: the shell still talks only to its local host. A layout
  leaf may name a pane on another host (`<host-id>/<pane>`); the local host
  fetches that pane's module and relays its msgs and events over an iroh
  connection (request/response stream), so the pane trait and the client
  never learn about the network.
- **Blobs** (`iroh-blobs`) for large assets later: meshes, textures, scene
  files by content hash. Not v1 of this milestone.
- **Membership**: a space is a ticket (space id + authority node id +
  relay); joining a space = dialling the ticket. Invite by sharing the
  ticket, like a share link today. Encryption and peer identity are iroh's.

Hooks already in the gallery build so this is a drop-in: the bus behind a
`Bus` trait (in-process impl now, `IrohBus` later), `{seq, origin}` on every
retained message, pane ids unique beyond one host.

Milestone order: gallery (#81) lands first, then `IrohBus` as its own
tracker item, behind a feature flag / config, with the in-process bus
staying the default for a single PC.

Open: iroh crate versions move fast (gossip/docs/blobs are separate crates
since iroh 0.28; check crates.io at build time); whether `iroh-docs` stays
maintained in its current shape, else retained topics go over gossip with
authority-stamped seq plus a sync request on join.

### IrohBus (built 2026-11-02, feature `iroh`)

`host::iroh_bus::IrohBus` wraps a `Local` bus; `cargo build -p host --features
iroh` adds it, the default build is unchanged. Pinned: `iroh` 1.4,
`iroh-gossip` 0.101.

- **One gossip topic per space** carries both kinds of message as JSON
  `{ topic, msg }`. `Local::tap` hands out what this host publishes;
  `Local::deliver` queues another host's for subscribers.
- **Retained topics: a replica over gossip, not iroh-docs.** iroh-docs pulls
  in iroh-blobs and a redb store, and keeps its own copy beside `bus.json`;
  the bus already has one small retained map. Instead `seq` is a Lamport
  count (one past the last, whichever host wrote it), a host keeps a copy
  only if it is newer by `(seq, origin)`, and every host sends its retained
  map to each new gossip neighbour. That converges without a second store.
- **Join**: `IrohBus::join(local, ticket, relays)`; `ticket()` is base32 of
  the gossip topic and this host's address. `Relays::Default` uses iroh's
  public relays and address lookup (hosts behind NAT); `Relays::Off` dials
  direct addresses only (LAN, tests).
- **Every ticket holder is trusted.** `Local::deliver` checks nothing a peer
  sends: not the topic's kind or prefix, not `check_size`, not who holds a
  dragged object. A peer can retain any topic up to the gossip limit
  (`MAX_WIRE`, 512 KiB, twice a local write's 256 KiB), and it is saved to
  `bus.json`. Share a ticket only with hosts you would let write the space.
- Tested: two in-process hosts, relays off: a retained topic written before
  the join converges, a transient message reaches the other host's
  subscriber, a later write on either side wins on both.

- **CLI** (feature `iroh`): `host --share <space>` opens that space on an
  `IrohBus`, prints its ticket and serves it at `/s/<space>/ticket`;
  `host --share <space> --join <ticket>` joins another host's; `--no-relays`
  for LAN/tests. Other spaces stay on `Local`. A joining host starts with
  nothing retained and does not seed: it takes the shared copy, which its
  old `bus.json` would otherwise beat wherever that has the higher seq.

Left: picking the bus from per-space config (today only the flag); command authority for standard topics (today every host applies commands
and the newest `scene` wins); remote pane modules and their msg/event relay;
blobs.

## Drags: streams, grabs and rollback (2026-11-02)

Cal's spec, hand-rolled (no ggrs, lightyear or naia):

- **Stream, then commit.** While dragging, view2d publishes the object on
  the transient `drag/<id>` once per pointer move; the drop is still one
  `move_object`, so `scene` only ever holds final positions. The host paces
  each `drag/<id>` to 60 Hz, holding back the latest (a connection wakes
  when it comes due and flushes it; idle connections never wake) and discarding a held one once the object is
  dropped or let go, so no sample lands after the final position.
- **Draw behind, snap forward.** Receivers draw a stream through
  `interpolate(topic)` in `web/protocol.js`: three samples, drawn one
  sample interval behind the newest, at most 100 ms of extrapolation when
  samples stop, and a snap (eased over 150 ms) when `scene` changes or
  `owners` drops the object. Positions only: objects have no rotation yet,
  so there is no slerp. `node crates/host/tests/interpolate.mjs` tests it.
- **Grab with rollback.** `grab`/`release` are commands, decided by the
  host in arrival order into the retained `owners` map (one map, since
  there are no wildcard subscriptions). The dragging pane predicts its grab
  from the first move; a lost grab comes back as an ordinary rejection of
  the `grab`, and the loser drops its prediction, flashes the object's
  outline and eases over to the holder's stream. Only the holder's drags
  pass the host, and only the holder may `move_object` a held object. A
  holder lets go on `release`, when its pane goes, or when the client
  connection that last drove its pane closes; a host start drops the grabs
  of that host's panes (only those, so a restart in a shared space keeps
  the other hosts' grabs).

Later sync slots in beside this rather than replacing it: every transient
message carries `{origin, ts}` (a monotonic clock per host) and every
retained one `{origin, seq}`, so a stream can be reordered and spaced per
origin and a retained topic's versions ordered by `(seq, origin)`, which is what a
CRDT (a per-object last-writer-wins register keyed by `(seq, origin)`) or a
lightyear-style replication layer needs as input. With `IrohBus`,
`drag/<id>` rides gossip as a transient topic, but grabs are still decided
per host: there is no single authority and nothing forwards a grab. Known
gap: two hosts can grant the same object at once, the whole `owners` map is
last-writer-wins by `(seq, origin)`, so one grant is lost silently, and
`Local::deliver` passes another host's `drag/<id>` without checking who
holds the object. Closing it needs an authority per object (or per space)
that other hosts forward grabs to.

## Future option: WASM panes (parked)

Cal wants this kept in mind in case the project pivots toward untrusted or
hot-loaded panes. The design leaves room: `Pane` has serde-only `Config`,
`Msg` and `Event` types and `Ctx` is tiny, so the same boundary can be
exposed as a WASM component interface (WIT) with the host as the embedder.
Keep pane state and messages plain data, no borrowed host types, so the move
costs a wrapper, not a rewrite. Revisit if sandboxing, third-party panes or
live loading become requirements.

## First milestone (proposed)

Host + client with: split/join/resize (client-first), two toy panes
(`pane-clock` as the stateless example, `pane-notes` with persisted text and
optimistic typing), layout save/load, `/pane/<id>` view working in a phone
browser, reconnect. Acceptance: `cargo tree -p pane-clock` shows only
`pane-api` and std-level deps from this workspace; drag-resize holds 60 fps
in both the Tauri window and a Chromium app window; typing in `pane-notes`
over the LAN shows no visible lag.

## Style

Rust, per ~/claude/style-study/rust-STYLE-STUDY.md. Small purpose-built types,
minimal API and error surface. Hand-written only; no code is derived from
`ai-gui` unless Cal asks (it stays in the attic).
