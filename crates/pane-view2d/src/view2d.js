// Front view of the scene (x right, y up) on a canvas. Pan, zoom, drag and hover hit-testing
// are local; the Rust half hears one message per click, drop, hover change and view at rest.
import { mount as toolbar } from '../widgets/toolbar.js';
import { Emits, Kind, isColor } from '../protocol.js';

export function mount(root, { send, onEvent }) {
  root.innerHTML = `<style>
    .wrap { display: flex; flex-direction: column; height: 100%; position: relative; }
    .tools { flex: none; }
    canvas { flex: 1; min-height: 0; width: 100%; display: block; touch-action: none;
             cursor: crosshair; background: #181818; }
    .note { position: absolute; left: 6px; bottom: 4px; font: 11px system-ui; color: #e6a700;
            pointer-events: none; }
  </style><div class="wrap"><div class="tools"></div><canvas></canvas><div class="note"></div></div>`;
  const canvas = root.querySelector('canvas');
  const note = root.querySelector('.note');
  const ctx = canvas.getContext('2d');
  let objects = [];
  let selected = new Set();
  let hovered = null;
  let view = { x: 0, y: 0, zoom: 40 }; // world units; zoom is pixels per unit
  let drag = null; // { kind: 'pan' | 'object', id, start, from, moved }
  let viewTimer = null;
  let noteTimer = null;

  const num = (v, d) => (Number.isFinite(v) ? v : d);
  const vec = (v) => (Array.isArray(v) ? [num(v[0], 0), num(v[1], 0), num(v[2], 0)] : [0, 0, 0]);
  const size = () => [canvas.clientWidth, canvas.clientHeight];
  const toScreen = (x, y) => {
    const [w, h] = size();
    return [w / 2 + (x - view.x) * view.zoom, h / 2 - (y - view.y) * view.zoom];
  };
  const object = (id) => objects.find((o) => o.id === id);
  const flash = (text) => {
    note.textContent = text;
    clearTimeout(noteTimer);
    noteTimer = setTimeout(() => { note.textContent = ''; }, 2500);
  };

  function draw() {
    const dpr = devicePixelRatio || 1;
    const [w, h] = size();
    if (canvas.width !== w * dpr || canvas.height !== h * dpr) {
      canvas.width = w * dpr;
      canvas.height = h * dpr;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    // Axes through the origin.
    const [ox, oy] = toScreen(0, 0);
    ctx.strokeStyle = '#2c2c2c';
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, oy); ctx.lineTo(w, oy);
    ctx.moveTo(ox, 0); ctx.lineTo(ox, h);
    ctx.stroke();
    for (const o of objects) {
      const [x, y] = vec(o.position);
      const [sx, sy] = vec(o.size);
      const [cx, cy] = toScreen(x, y);
      const pw = Math.max(2, sx * view.zoom);
      const ph = Math.max(2, sy * view.zoom);
      ctx.fillStyle = isColor(o.color) ? o.color : '#888';
      ctx.beginPath();
      if (o.kind === Kind.SPHERE) ctx.ellipse(cx, cy, pw / 2, ph / 2, 0, 0, 2 * Math.PI);
      else if (o.kind === Kind.LIGHT) {
        ctx.moveTo(cx, cy - ph / 2); ctx.lineTo(cx + pw / 2, cy);
        ctx.lineTo(cx, cy + ph / 2); ctx.lineTo(cx - pw / 2, cy); ctx.closePath();
      } else ctx.rect(cx - pw / 2, cy - ph / 2, pw, ph);
      ctx.fill();
      if (selected.has(o.id) || hovered === o.id) {
        ctx.lineWidth = selected.has(o.id) ? 2 : 1;
        ctx.strokeStyle = selected.has(o.id) ? '#fff' : '#aaa';
        ctx.stroke();
      }
    }
  }

  // The topmost object under a canvas point, or null.
  function hit(px, py) {
    for (let i = objects.length - 1; i >= 0; i--) {
      const o = objects[i];
      const [x, y] = vec(o.position);
      const [sx, sy] = vec(o.size);
      const [cx, cy] = toScreen(x, y);
      const dx = (px - cx) / Math.max(2, sx * view.zoom) * 2;
      const dy = (py - cy) / Math.max(2, sy * view.zoom) * 2;
      const inside = o.kind === Kind.SPHERE ? dx * dx + dy * dy <= 1 : Math.abs(dx) <= 1 && Math.abs(dy) <= 1;
      if (inside) return o.id;
    }
    return null;
  }

  const settleView = () => {
    clearTimeout(viewTimer);
    viewTimer = null;
    const round = (v) => Math.round(v * 1000) / 1000;
    send({ SetView: { view: { x: round(view.x), y: round(view.y), zoom: round(view.zoom) } } });
  };
  const applyView = (v) => {
    if (!v || typeof v !== 'object') return;
    view = { x: num(v.x, view.x), y: num(v.y, view.y), zoom: Math.min(400, Math.max(2, num(v.zoom, view.zoom))) };
  };
  const fit = () => {
    if (!objects.length) return;
    let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
    for (const o of objects) {
      const [x, y] = vec(o.position);
      const [sx, sy] = vec(o.size);
      x0 = Math.min(x0, x - sx / 2); x1 = Math.max(x1, x + sx / 2);
      y0 = Math.min(y0, y - sy / 2); y1 = Math.max(y1, y + sy / 2);
    }
    const [w, h] = size();
    const zoom = Math.min(w / Math.max(1, x1 - x0), h / Math.max(1, y1 - y0)) * 0.8;
    applyView({ x: (x0 + x1) / 2, y: (y0 + y1) / 2, zoom });
    draw();
    settleView();
  };

  canvas.addEventListener('pointerdown', (e) => {
    canvas.setPointerCapture(e.pointerId);
    const id = hit(e.offsetX, e.offsetY);
    const start = [e.offsetX, e.offsetY];
    drag = id
      ? { kind: 'object', id, start, from: vec(object(id).position), moved: false }
      : { kind: 'pan', start, from: [view.x, view.y], moved: false };
  });
  canvas.addEventListener('pointermove', (e) => {
    if (!drag) {
      const id = hit(e.offsetX, e.offsetY);
      if (id === hovered) return;
      hovered = id;
      send({ Hover: { id } });
      draw();
      return;
    }
    const dx = (e.offsetX - drag.start[0]) / view.zoom;
    const dy = -(e.offsetY - drag.start[1]) / view.zoom;
    if (Math.hypot(dx, dy) * view.zoom > 3) drag.moved = true;
    if (!drag.moved) return;
    if (drag.kind === 'pan') {
      view.x = drag.from[0] - dx;
      view.y = drag.from[1] - dy;
    } else {
      drag.position = [drag.from[0] + dx, drag.from[1] + dy, drag.from[2]];
      const o = object(drag.id);
      if (o) o.position = drag.position;
    }
    draw();
  });
  canvas.addEventListener('pointerup', () => {
    if (!drag) return;
    if (!drag.moved) {
      const ids = drag.kind === 'object' ? [drag.id] : [];
      selected = new Set(ids);
      send({ Select: { ids } });
    } else if (drag.kind === 'object') {
      send({ Move: { id: drag.id, position: drag.position } });
    } else settleView();
    drag = null;
    draw();
  });
  canvas.addEventListener('wheel', (e) => {
    e.preventDefault();
    const factor = Math.exp(-e.deltaY * 0.002);
    const [w, h] = size();
    // Zoom about the cursor: the world point under it stays put.
    const wx = (e.offsetX - w / 2) / view.zoom + view.x;
    const wy = -(e.offsetY - h / 2) / view.zoom + view.y;
    const zoom = Math.min(400, Math.max(2, view.zoom * factor));
    view = { x: wx - (e.offsetX - w / 2) / zoom, y: wy + (e.offsetY - h / 2) / zoom, zoom };
    draw();
    clearTimeout(viewTimer);
    viewTimer = setTimeout(settleView, 300);
  }, { passive: false });

  const tools = toolbar(root.querySelector('.tools'), {
    items: [
      { name: 'fit', label: 'fit', title: 'show everything' },
      { name: 'clear', label: 'clear', title: 'select nothing' },
    ],
  }, (name, { name: action }) => {
    if (name !== Emits.ACTION) return;
    if (action === 'fit') fit();
    if (action === 'clear') {
      selected = new Set();
      send({ Select: { ids: [] } });
      draw();
    }
  });

  onEvent((e) => {
    if (e.State) {
      objects = e.State.scene?.objects ?? [];
      selected = new Set(e.State.selection?.ids ?? []);
      applyView(e.State.view);
    } else if (e.Scene) {
      objects = e.Scene.scene?.objects ?? [];
      // Keep the object in hand where the pointer has it.
      const o = drag?.position && object(drag.id);
      if (o) o.position = drag.position;
    } else if (e.Selection) selected = new Set(e.Selection.selection?.ids ?? []);
    else if (e.Hover) hovered = e.Hover.hover?.id ?? null;
    else if (e.Focus) {
      const o = object(e.Focus.id);
      if (o) {
        const [x, y] = vec(o.position);
        applyView({ x, y, zoom: view.zoom });
        settleView();
      }
    } else if (e.View) {
      if (!drag && viewTimer === null) applyView(e.View.view);
    } else if (e.Rejected) flash(e.Rejected.reason);
    else if (e.Unsupported) flash(`protocol version ${JSON.stringify(e.Unsupported.version)} is not one this pane knows`);
    draw();
  });

  const observer = new ResizeObserver(draw);
  observer.observe(canvas);
  draw();
  return () => {
    observer.disconnect();
    tools.destroy();
    clearTimeout(viewTimer);
    clearTimeout(noteTimer);
  };
}
