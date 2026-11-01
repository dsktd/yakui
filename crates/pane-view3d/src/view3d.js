// The scene in WebGL2: boxes and spheres with flat shading, an orbit camera, and picking by
// rendering ids as colours. Orbit, zoom and hover picking are local; the Rust half hears one
// message per click, hover change and camera at rest.
import { mount as toolbar } from '../widgets/toolbar.js';
import { Emits, Kind, isColor } from '../protocol.js';
import { flash } from '../note.js';

const VS = `#version 300 es
in vec3 position;
uniform mat4 mvp;
uniform mat4 model;
out vec3 world;
void main() {
  world = (model * vec4(position, 1.0)).xyz;
  gl_Position = mvp * vec4(position, 1.0);
}`;
const FS = `#version 300 es
precision mediump float;
in vec3 world;
uniform vec3 color;
uniform bool unlit;
out vec4 out_color;
void main() {
  if (unlit) { out_color = vec4(color, 1.0); return; }
  vec3 n = normalize(cross(dFdx(world), dFdy(world)));
  float light = 0.35 + 0.65 * abs(dot(n, normalize(vec3(0.5, 0.8, 0.6))));
  out_color = vec4(color * light, 1.0);
}`;

// --- small matrix kit, column-major Float32Array ---
function perspective(fov, aspect, near, far) {
  const f = 1 / Math.tan(fov / 2);
  const m = new Float32Array(16);
  m[0] = f / aspect; m[5] = f; m[10] = (far + near) / (near - far); m[11] = -1;
  m[14] = (2 * far * near) / (near - far);
  return m;
}
function lookAt(eye, target, up) {
  const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
  const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
  const norm = (a) => { const l = Math.hypot(...a) || 1; return [a[0] / l, a[1] / l, a[2] / l]; };
  const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
  const z = norm(sub(eye, target));
  const x = norm(cross(up, z));
  const y = cross(z, x);
  return new Float32Array([
    x[0], y[0], z[0], 0,
    x[1], y[1], z[1], 0,
    x[2], y[2], z[2], 0,
    -dot(x, eye), -dot(y, eye), -dot(z, eye), 1,
  ]);
}
function multiply(a, b) {
  const m = new Float32Array(16);
  for (let c = 0; c < 4; c++) for (let r = 0; r < 4; r++) {
    m[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] + a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
  }
  return m;
}
function translateScale(p, s) {
  return new Float32Array([s[0], 0, 0, 0, 0, s[1], 0, 0, 0, 0, s[2], 0, p[0], p[1], p[2], 1]);
}

// --- meshes: unit size, centred, plain triangle lists ---
function cube() {
  const v = [];
  const face = (a, b, c, d) => v.push(...a, ...b, ...c, ...a, ...c, ...d);
  const h = 0.5;
  face([-h, -h, h], [h, -h, h], [h, h, h], [-h, h, h]);
  face([h, -h, -h], [-h, -h, -h], [-h, h, -h], [h, h, -h]);
  face([-h, h, h], [h, h, h], [h, h, -h], [-h, h, -h]);
  face([-h, -h, -h], [h, -h, -h], [h, -h, h], [-h, -h, h]);
  face([h, -h, h], [h, -h, -h], [h, h, -h], [h, h, h]);
  face([-h, -h, -h], [-h, -h, h], [-h, h, h], [-h, h, -h]);
  return new Float32Array(v);
}
function sphere(rings = 12, segments = 16) {
  const p = (i, j) => {
    const phi = (i / rings) * Math.PI;
    const theta = (j / segments) * 2 * Math.PI;
    return [0.5 * Math.sin(phi) * Math.cos(theta), 0.5 * Math.cos(phi), 0.5 * Math.sin(phi) * Math.sin(theta)];
  };
  const v = [];
  // Counter-clockwise seen from outside, like the cube, so culling keeps the near side.
  for (let i = 0; i < rings; i++) for (let j = 0; j < segments; j++) {
    const a = p(i, j), b = p(i + 1, j), c = p(i + 1, j + 1), d = p(i, j + 1);
    v.push(...a, ...c, ...b, ...a, ...d, ...c);
  }
  return new Float32Array(v);
}

export function mount(root, { send, onEvent }) {
  root.innerHTML = `<style>
    .wrap { display: flex; flex-direction: column; height: 100%; position: relative; }
    .tools { flex: none; }
    canvas { flex: 1; min-height: 0; width: 100%; display: block; touch-action: none;
             cursor: grab; background: #181818; }
    .note { position: absolute; left: 6px; bottom: 4px; font: 11px system-ui; color: #e6a700;
            pointer-events: none; }
  </style><div class="wrap"><div class="tools"></div><canvas></canvas><div class="note"></div></div>`;
  const canvas = root.querySelector('canvas');
  const note = root.querySelector('.note');
  const gl = canvas.getContext('webgl2');
  if (!gl) {
    note.textContent = 'WebGL2 is not available here';
    return () => {};
  }
  let objects = [];
  let selected = new Set();
  let hovered = null;
  let camera = { yaw: 0.6, pitch: 0.4, distance: 12, target: [0, 0, 0] };
  let drag = null; // { start, from, moved }
  let frame = null;
  let cameraTimer = null;
  let pendingHover = null;

  const num = (v, d) => (Number.isFinite(v) ? v : d);
  const vec = (v, d) => (Array.isArray(v) ? [num(v[0], d), num(v[1], d), num(v[2], d)] : [d, d, d]);

  // --- GL setup ---
  const shader = (type, source) => {
    const s = gl.createShader(type);
    gl.shaderSource(s, source);
    gl.compileShader(s);
    return s;
  };
  const program = gl.createProgram();
  gl.attachShader(program, shader(gl.VERTEX_SHADER, VS));
  gl.attachShader(program, shader(gl.FRAGMENT_SHADER, FS));
  gl.bindAttribLocation(program, 0, 'position');
  gl.linkProgram(program);
  gl.useProgram(program);
  const u = {
    mvp: gl.getUniformLocation(program, 'mvp'),
    model: gl.getUniformLocation(program, 'model'),
    color: gl.getUniformLocation(program, 'color'),
    unlit: gl.getUniformLocation(program, 'unlit'),
  };
  const meshes = {};
  for (const [kind, data] of [[Kind.BOX, cube()], [Kind.SPHERE, sphere()]]) {
    const vao = gl.createVertexArray();
    gl.bindVertexArray(vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 0, 0);
    meshes[kind] = { vao, count: data.length / 3 };
  }
  gl.enable(gl.DEPTH_TEST);
  gl.enable(gl.CULL_FACE);

  const hex = (c) => [parseInt(c.slice(1, 3), 16) / 255, parseInt(c.slice(3, 5), 16) / 255, parseInt(c.slice(5, 7), 16) / 255];
  const mix = (c, t) => c.map((x) => x + (1 - x) * t);
  const eye = () => {
    const { yaw, pitch, distance, target } = camera;
    return [
      target[0] + distance * Math.cos(pitch) * Math.sin(yaw),
      target[1] + distance * Math.sin(pitch),
      target[2] + distance * Math.cos(pitch) * Math.cos(yaw),
    ];
  };

  // Draw every object; in `pick` mode as its index encoded in the colour, unlit.
  function render(pick) {
    const dpr = devicePixelRatio || 1;
    const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    gl.viewport(0, 0, w, h);
    gl.clearColor(pick ? 0 : 0.094, pick ? 0 : 0.094, pick ? 0 : 0.094, 1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    const view = multiply(perspective(0.9, w / h, 0.1, 500), lookAt(eye(), camera.target, [0, 1, 0]));
    objects.forEach((o, i) => {
      const mesh = meshes[o.kind === Kind.SPHERE ? Kind.SPHERE : Kind.BOX];
      const model = translateScale(vec(o.position, 0), vec(o.size, 1).map((s) => Math.max(0.01, s)));
      gl.uniformMatrix4fv(u.mvp, false, multiply(view, model));
      gl.uniformMatrix4fv(u.model, false, model);
      let color = isColor(o.color) ? hex(o.color) : [0.5, 0.5, 0.5];
      if (pick) color = [((i + 1) & 255) / 255, ((i + 1) >> 8 & 255) / 255, 0];
      else if (selected.has(o.id)) color = mix(color, 0.5);
      else if (hovered === o.id) color = mix(color, 0.25);
      gl.uniform3fv(u.color, color);
      gl.uniform1i(u.unlit, pick || o.kind === Kind.LIGHT ? 1 : 0);
      gl.bindVertexArray(mesh.vao);
      gl.drawArrays(gl.TRIANGLES, 0, mesh.count);
    });
  }
  const draw = () => {
    if (frame !== null) return;
    frame = requestAnimationFrame(() => {
      frame = null;
      render(false);
      if (pendingHover) {
        const [x, y] = pendingHover;
        pendingHover = null;
        const id = pick(x, y);
        if (id !== hovered) {
          hovered = id;
          send({ Hover: { id } });
          render(false);
        }
      }
    });
  };
  // The object under a canvas point, read back from an id-coloured frame.
  function pick(px, py) {
    render(true);
    const dpr = devicePixelRatio || 1;
    const out = new Uint8Array(4);
    gl.readPixels(Math.round(px * dpr), canvas.height - Math.round(py * dpr) - 1, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, out);
    const index = out[0] + (out[1] << 8) - 1;
    render(false);
    return index >= 0 && index < objects.length ? objects[index].id : null;
  }

  const settleCamera = () => {
    clearTimeout(cameraTimer);
    cameraTimer = null;
    const round = (v) => Math.round(v * 1000) / 1000;
    const { yaw, pitch, distance, target } = camera;
    send({ SetCamera: { camera: { yaw: round(yaw), pitch: round(pitch), distance: round(distance), target: target.map(round) } } });
  };
  const applyCamera = (c) => {
    if (!c || typeof c !== 'object') return;
    camera = {
      yaw: num(c.yaw, camera.yaw),
      pitch: Math.min(1.5, Math.max(-1.5, num(c.pitch, camera.pitch))),
      distance: Math.min(200, Math.max(0.5, num(c.distance, camera.distance))),
      target: vec(c.target, 0),
    };
  };

  canvas.addEventListener('pointerdown', (e) => {
    canvas.setPointerCapture(e.pointerId);
    drag = { start: [e.offsetX, e.offsetY], from: [camera.yaw, camera.pitch], moved: false };
  });
  canvas.addEventListener('pointermove', (e) => {
    if (!drag) {
      pendingHover = [e.offsetX, e.offsetY];
      draw();
      return;
    }
    const dx = e.offsetX - drag.start[0];
    const dy = e.offsetY - drag.start[1];
    if (Math.hypot(dx, dy) > 3) drag.moved = true;
    if (!drag.moved) return;
    applyCamera({ ...camera, yaw: drag.from[0] - dx * 0.01, pitch: drag.from[1] + dy * 0.01 });
    draw();
  });
  canvas.addEventListener('pointerup', (e) => {
    if (!drag) return;
    if (!drag.moved) {
      const id = pick(e.offsetX, e.offsetY);
      const ids = id ? [id] : [];
      selected = new Set(ids);
      send({ Select: { ids } });
    } else settleCamera();
    drag = null;
    draw();
  });
  canvas.addEventListener('wheel', (e) => {
    e.preventDefault();
    applyCamera({ ...camera, distance: camera.distance * Math.exp(e.deltaY * 0.002) });
    draw();
    clearTimeout(cameraTimer);
    cameraTimer = setTimeout(settleCamera, 300);
  }, { passive: false });

  const tools = toolbar(root.querySelector('.tools'), {
    items: [
      { name: 'reset', label: 'reset', title: 'camera back to the start' },
      { name: 'clear', label: 'clear', title: 'select nothing' },
    ],
  }, (name, { name: action }) => {
    if (name !== Emits.ACTION) return;
    if (action === 'reset') {
      applyCamera({ yaw: 0.6, pitch: 0.4, distance: 12, target: [0, 0, 0] });
      draw();
      settleCamera();
    }
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
      applyCamera(e.State.camera);
    } else if (e.Scene) objects = e.Scene.scene?.objects ?? [];
    else if (e.Selection) selected = new Set(e.Selection.selection?.ids ?? []);
    else if (e.Hover) hovered = e.Hover.hover?.id ?? null;
    else if (e.Focus) {
      const o = objects.find((o) => o.id === e.Focus.id);
      if (o) {
        applyCamera({ ...camera, target: vec(o.position, 0) });
        settleCamera();
      }
    } else if (e.Camera) {
      if (!drag && cameraTimer === null) applyCamera(e.Camera.camera);
    } else if (e.Rejected) flash(note, e.Rejected.reason);
    else if (e.Unsupported) flash(note, `protocol version ${JSON.stringify(e.Unsupported.version)} is not one this pane knows`);
    draw();
  });

  const observer = new ResizeObserver(draw);
  observer.observe(canvas);
  draw();
  return () => {
    observer.disconnect();
    tools.destroy();
    if (frame !== null) cancelAnimationFrame(frame);
    clearTimeout(cameraTimer);
  };
}
