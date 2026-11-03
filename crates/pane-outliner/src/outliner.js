// The scene as a tree grouped by kind. A click on an object is a Select, double click a
// Focus, the pointer coming onto or leaving an object a Hover; group rows do nothing.
import { mount as tree } from 'yakui/widgets/tree.js';
import { Emits } from 'yakui/protocol.js';
import { flash } from 'yakui/note.js';

export function mount(root, { send, onEvent }) {
  root.innerHTML = `<style>
    .note { font: 11px system-ui; color: #e6a700; padding: 2px 6px; min-height: 1.2em; }
  </style><div class="tree"></div><div class="note"></div>`;
  const note = root.querySelector('.note');
  let objects = [];
  let selected = [];
  let hovered = null;
  let sentHover = null;
  const isObject = (id) => objects.some((o) => o.id === id);

  const nodes = () => {
    const groups = new Map();
    for (const o of objects) {
      const kind = o.kind || 'other';
      if (!groups.has(kind)) groups.set(kind, []);
      groups.get(kind).push({ id: o.id, label: o.name || o.id });
    }
    return [...groups].map(([kind, children]) => ({ id: `kind:${kind}`, label: `${kind} (${children.length})`, children }));
  };
  const render = () => widget.update({ nodes: nodes(), selected, hovered });

  const widget = tree(root.querySelector('.tree'), { nodes: [] }, (name, payload) => {
    if (name === Emits.SELECT) {
      const ids = payload.ids.filter(isObject);
      if (!ids.length) return;
      selected = ids;
      send({ Select: { ids } });
      render();
    } else if (name === Emits.HOVER) {
      const id = isObject(payload.id) ? payload.id : null;
      if (id === sentHover) return;
      sentHover = id;
      send({ Hover: { id } });
    } else if (name === Emits.PICK && isObject(payload.value)) {
      send({ Focus: { id: payload.value } });
    }
  });

  onEvent((e) => {
    if (e.State) {
      objects = e.State.scene?.objects ?? [];
      selected = e.State.selection?.ids ?? [];
    } else if (e.Hover) hovered = e.Hover.hover?.id ?? null;
    else if (e.Rejected) flash(note, e.Rejected.reason);
    else if (e.Unsupported) flash(note, `protocol version ${JSON.stringify(e.Unsupported.version)} is not one this pane knows`);
    render();
  });
  return () => widget.destroy();
}
