// The shell: one WebSocket, the layout tree, and a shadow root per pane.
// Layout ops apply here first; the host answers with the whole layout.

const single = location.pathname.match(/^\/pane\/(\d+)$/);
const only = single ? Number(single[1]) : null;
const roParam = new URLSearchParams(location.search).get('ro');
const ro = roParam !== null && roParam !== '0' && roParam !== 'false';
const strip = document.getElementById('strip');
const area = document.getElementById('layout');

let ws = null;
let retry = 500;
let layout = null;
let kinds = [];
const panes = new Map();   // id -> { el, kind, listeners, pending, unmount, select }
const handles = new Map(); // path string -> element

function connect() {
  const q = new URLSearchParams();
  if (only !== null) q.set('pane', only);
  if (ro) q.set('ro', '1');
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  ws = new WebSocket(`${proto}://${location.host}/ws?${q}`);
  ws.onopen = () => { strip.hidden = true; retry = 500; };
  ws.onclose = () => {
    strip.hidden = false;
    setTimeout(connect, retry);
    retry = Math.min(retry * 2, 5000);
  };
  ws.onmessage = (e) => receive(JSON.parse(e.data));
}

function send(frame) {
  if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(frame));
}

function receive(frame) {
  if ('layout' in frame) {
    layout = frame.layout;
    kinds = frame.kinds;
    render();
    return;
  }
  const pane = panes.get(frame.pane_id);
  if (!pane) return;
  // The module is still importing: hold its snapshot until it calls onEvent.
  if (pane.pending) pane.pending.push(frame.payload);
  else for (const cb of pane.listeners) cb(frame.payload);
}

// --- layout ops, mirrored from the host so they apply instantly ---

function findLeaf(node, pane) {
  if ('Leaf' in node) return node.Leaf.pane === pane ? node : null;
  return findLeaf(node.Split.a, pane) || findLeaf(node.Split.b, pane);
}

function findParent(node, pane) {
  if ('Leaf' in node) return null;
  const { a, b } = node.Split;
  if (('Leaf' in a && a.Leaf.pane === pane) || ('Leaf' in b && b.Leaf.pane === pane)) return node;
  return findParent(a, pane) || findParent(b, pane);
}

// The split holding leaf `pane` and its other child, or null at the root.
function sibling(pane) {
  const parent = findParent(layout.root, pane);
  if (!parent) return null;
  const { a, b } = parent.Split;
  return { parent, other: 'Leaf' in a && a.Leaf.pane === pane ? b : a };
}

function apply(op) {
  if ('Split' in op) {
    const { pane, axis, new_pane, kind } = op.Split;
    const leaf = findLeaf(layout.root, pane);
    if (!leaf) return;
    const a = { Leaf: { pane } };
    const b = { Leaf: { pane: new_pane } };
    delete leaf.Leaf;
    leaf.Split = { axis, ratio: 0.5, a, b };
    layout.panes[new_pane] = { kind, config: null };
  } else if ('Join' in op) {
    const { pane } = op.Join;
    const s = sibling(pane);
    if (!s) return;
    delete s.parent.Split;
    Object.assign(s.parent, s.other);
    delete layout.panes[pane];
  } else if ('Swap' in op) {
    const { a, b } = op.Swap;
    const la = findLeaf(layout.root, a);
    const lb = findLeaf(layout.root, b);
    if (!la || !lb) return;
    la.Leaf.pane = b;
    lb.Leaf.pane = a;
  } else if ('Change' in op) {
    layout.panes[op.Change.pane].kind = op.Change.kind;
  }
}

function op(o) {
  if (ro) return;
  apply(o);
  render();
  send({ layout: o });
}

function freshId() {
  let id;
  do { id = crypto.getRandomValues(new Uint32Array(1))[0]; } while (id === 0 || layout.panes[id]);
  return id;
}

// --- rendering ---

function render() {
  if (!layout) return;
  const seen = new Set();
  const seenHandles = new Set();
  const full = { x: 0, y: 0, w: 1, h: 1 };
  if (only !== null) {
    if (layout.panes[only]) place(only, full, seen);
  } else {
    walk(layout.root, full, [], seen, seenHandles);
  }
  for (const [id, pane] of panes) {
    if (seen.has(id)) continue;
    pane.unmount?.();
    pane.el.remove();
    panes.delete(id);
  }
  for (const [key, el] of handles) {
    if (seenHandles.has(key)) continue;
    el.remove();
    handles.delete(key);
  }
}

function walk(node, rect, path, seen, seenHandles) {
  if ('Leaf' in node) return place(node.Leaf.pane, rect, seen);
  const { axis, ratio, a, b } = node.Split;
  const vertical = axis === 'Vertical';
  const ra = vertical ? { ...rect, w: rect.w * ratio } : { ...rect, h: rect.h * ratio };
  const rb = vertical
    ? { ...rect, x: rect.x + rect.w * ratio, w: rect.w * (1 - ratio) }
    : { ...rect, y: rect.y + rect.h * ratio, h: rect.h * (1 - ratio) };
  walk(a, ra, [...path, 'A'], seen, seenHandles);
  walk(b, rb, [...path, 'B'], seen, seenHandles);
  if (ro) return;
  const key = path.join('');
  seenHandles.add(key);
  let el = handles.get(key);
  if (!el) {
    el = document.createElement('div');
    el.addEventListener('pointerdown', (e) => drag(e, el));
    area.append(el);
    handles.set(key, el);
  }
  el.className = `handle ${vertical ? 'v' : 'h'}`;
  el.path = path;
  el.rect = rect;
  el.vertical = vertical;
  el.node = node;
  if (vertical) {
    setRect(el, { x: rect.x + rect.w * ratio, y: rect.y, w: 0, h: rect.h });
    el.style.width = '';
  } else {
    setRect(el, { x: rect.x, y: rect.y + rect.h * ratio, w: rect.w, h: 0 });
    el.style.height = '';
  }
}

function setRect(el, r) {
  el.style.left = `${r.x * 100}%`;
  el.style.top = `${r.y * 100}%`;
  el.style.width = `${r.w * 100}%`;
  el.style.height = `${r.h * 100}%`;
}

function drag(e, el) {
  el.setPointerCapture(e.pointerId);
  const box = area.getBoundingClientRect();
  const move = (e) => {
    const { rect, vertical } = el;
    const ratio = vertical
      ? ((e.clientX - box.left) / box.width - rect.x) / rect.w
      : ((e.clientY - box.top) / box.height - rect.y) / rect.h;
    el.node.Split.ratio = Math.min(0.95, Math.max(0.05, ratio));
    render();
  };
  const up = () => {
    el.removeEventListener('pointermove', move);
    el.removeEventListener('pointerup', up);
    send({ layout: { Resize: { path: el.path, ratio: el.node.Split.ratio } } });
  };
  el.addEventListener('pointermove', move);
  el.addEventListener('pointerup', up);
}

function place(id, rect, seen) {
  seen.add(id);
  const kind = layout.panes[id].kind;
  let pane = panes.get(id);
  if (pane && pane.kind !== kind) {
    pane.unmount?.();
    pane.el.remove();
    pane = null;
  }
  if (!pane) {
    pane = mount(id, kind);
    panes.set(id, pane);
  }
  setRect(pane.el, rect);
  if (pane.select) pane.select.value = kind;
}

function mount(id, kind) {
  const el = document.createElement('div');
  el.className = only !== null ? 'pane single' : 'pane';
  const pane = { el, kind, listeners: new Set(), pending: [], unmount: null, select: null };
  if (only === null && !ro) el.append(bar(id, kind, pane));
  const body = document.createElement('div');
  body.className = 'body';
  el.append(body);
  const shadow = body.attachShadow({ mode: 'closed' });
  shadow.innerHTML = '<style>:host { display: block; } .root { height: 100%; }</style><div class="root"></div>';
  const root = shadow.querySelector('.root');
  area.append(el);
  const api = {
    send: (payload) => { if (!ro) send({ pane_id: id, payload }); },
    onEvent: (cb) => {
      pane.listeners.add(cb);
      const held = pane.pending || [];
      pane.pending = null;
      for (const payload of held) cb(payload);
    },
  };
  import(`/modules/${kind}`).then((mod) => {
    if (panes.get(id) !== pane) return;
    pane.unmount = mod.mount(root, api) || null;
    pane.pending = null;
  }).catch((err) => { root.textContent = String(err); pane.pending = null; });
  return pane;
}

function bar(id, kind, pane) {
  const el = document.createElement('div');
  el.className = 'bar';
  const select = document.createElement('select');
  for (const k of kinds) select.append(new Option(k, k));
  select.value = kind;
  select.onchange = () => op({ Change: { pane: id, kind: select.value } });
  pane.select = select;
  const grow = document.createElement('div');
  grow.className = 'grow';
  const button = (label, title, fn) => {
    const b = document.createElement('button');
    b.textContent = label;
    b.title = title;
    b.onclick = fn;
    return b;
  };
  const split = (axis) => op({ Split: { pane: id, axis, new_pane: freshId(), kind: layout.panes[id].kind } });
  const swap = () => {
    const s = sibling(id);
    if (s && 'Leaf' in s.other) op({ Swap: { a: id, b: s.other.Leaf.pane } });
  };
  el.append(select, grow,
    button('|', 'split vertical', () => split('Vertical')),
    button('—', 'split horizontal', () => split('Horizontal')),
    button('⇄', 'swap with sibling', swap),
    button('×', 'join into sibling', () => op({ Join: { pane: id } })));
  return el;
}

connect();
