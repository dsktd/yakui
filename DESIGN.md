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
  link joins that space; a "share" action in the shell shows the link. `/`
  redirects to the client's own space, creating it on first visit.
- Read-only stays a per-connection flag (`?ro=1`), so a shared link can be a
  passive mirror.
- The Tauri window is a webview with its own storage, so a separate client
  (and space) from the desktop browser unless given a link.
- Later, if wanted: a page listing spaces, renaming, deleting an empty one.
  Not v1.

Name "space" is a placeholder for Cal to replace.

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

URLs: `/` full layout, `/pane/<id>` one pane filling the viewport (the mobile
case), `/ws`. The web client is one small shell that mounts each pane's ES
module into its rect.

### Remote access and auth

Bind 127.0.0.1 by default. LAN or remote exposure only behind an
authenticating proxy (same rule as the Caddy proxy here). Read-only vs
read-write is a per-connection flag so a phone can view without the ability to
send messages.

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
