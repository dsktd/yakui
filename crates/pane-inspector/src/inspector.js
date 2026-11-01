// The selected object's fields in a property list. A committed field is one Set message;
// the object comes back through the scene, so nothing here is state of its own.
import { mount as propertyList } from '../widgets/property-list.js';
import { Emits } from '../protocol.js';
import { flash } from '../note.js';

export function mount(root, { send, onEvent }) {
  root.innerHTML = `<style>
    .head { font: 12px system-ui; color: #999; padding: 6px 6px 0; }
    .head b { color: #ddd; }
    .note { font: 11px system-ui; color: #e6a700; padding: 0 6px 4px; min-height: 1.2em; }
  </style><div class="head">nothing selected</div><div class="list"></div><div class="note"></div>`;
  const head = root.querySelector('.head');
  const note = root.querySelector('.note');
  let object = null;

  const rows = (o) => {
    if (!o) return [];
    const axis = (field, labels) => labels.map((label, i) => ({
      key: `${field}.${i}`, label, kind: 'number', value: o[field]?.[i] ?? 0, step: 0.1,
    }));
    const props = Object.entries(o.props ?? {}).map(([k, v]) => ({
      key: `props.${k}`, label: k,
      kind: typeof v === 'number' ? 'number' : 'text',
      value: typeof v === 'number' ? v : JSON.stringify(v),
    }));
    return [
      { key: 'name', label: 'name', kind: 'text', value: o.name },
      { key: 'kind', label: 'kind', kind: 'text', value: o.kind },
      ...axis('position', ['x', 'y', 'z']),
      ...axis('size', ['width', 'height', 'depth']),
      { key: 'color', label: 'colour', kind: 'color', value: o.color },
      ...props,
    ];
  };

  const list = propertyList(root.querySelector('.list'), { rows: [] }, (name, { key, value }) => {
    if (name !== Emits.CHANGE || !object) return;
    const dot = key.indexOf('.');
    const field = dot < 0 ? key : key.slice(0, dot);
    const rest = dot < 0 ? null : key.slice(dot + 1);
    if (field === 'position' || field === 'size') {
      const v = [...(object[field] ?? [0, 0, 0])];
      v[Number(rest)] = value;
      send({ Set: { id: object.id, key: field, value: v } });
    } else if (field === 'props') {
      let parsed = value;
      if (typeof value === 'string') {
        try { parsed = JSON.parse(value); } catch { parsed = value; }
      }
      send({ Set: { id: object.id, key: rest, value: parsed } });
    } else send({ Set: { id: object.id, key, value } });
  });

  onEvent((e) => {
    if (e.Object) {
      object = e.Object.object ?? null;
      head.innerHTML = object ? `<b></b> <span></span>` : 'nothing selected';
      if (object) {
        head.querySelector('b').textContent = object.name || object.id;
        head.querySelector('span').textContent = object.id;
      }
      list.update({ rows: rows(object) });
    } else if (e.Rejected) flash(note, e.Rejected.reason);
    else if (e.Unsupported) flash(note, `protocol version ${JSON.stringify(e.Unsupported.version)} is not one this pane knows`);
  });
  return () => list.destroy();
}
