// Labelled fields, one per row, each edited by the widget for its kind. Rows are keyed, so
// an update re-renders in place and a field being edited keeps its text.
// props: { rows: [{ key, label, kind: "text" | "number" | "color", value, min?, max?, step? }] }
// emits: change { key, value } (from the row's widget; a text row commits on change)
import { mount as numberField } from './number-field.js';
import { mount as colorSwatch } from './color-swatch.js';

export function mount(el, props, emit) {
  const shadow = el.attachShadow({ mode: 'open' });
  shadow.innerHTML = `<style>
    .list { display: flex; flex-direction: column; gap: 4px; padding: 6px; }
    .list:empty::after { content: 'nothing'; color: #777; font: 12px system-ui; }
    label { display: flex; align-items: center; gap: 6px; font: 12px system-ui; color: #bbb; }
    label span { flex: 1; }
    label input { width: 10em; font: inherit; background: #1a1a1a; color: #eee;
                  border: 1px solid #444; border-radius: 2px; padding: 2px 4px; }
  </style><div class="list"></div>`;
  const list = shadow.querySelector('.list');
  const rows = new Map(); // key -> { kind, el, update, destroy }

  const textField = (holder, row) => {
    holder.innerHTML = '<label><span></span><input></label>';
    const text = holder.querySelector('span');
    const input = holder.querySelector('input');
    const commit = () => emit('change', { key: row.key, value: input.value });
    input.addEventListener('change', commit);
    const update = (row) => {
      text.textContent = row.label ?? row.key;
      if (shadow.activeElement !== input) input.value = row.value ?? '';
    };
    update(row);
    return { update, destroy: () => input.removeEventListener('change', commit) };
  };
  const field = (holder, row) => {
    if (row.kind === 'number') return numberField(holder, row, emit);
    if (row.kind === 'color') return colorSwatch(holder, { ...row, color: row.value }, emit);
    return textField(holder, row);
  };
  const render = (props) => {
    const seen = new Set();
    for (const row of props.rows ?? []) {
      seen.add(row.key);
      let r = rows.get(row.key);
      if (r && r.kind !== row.kind) {
        r.destroy();
        r.el.remove();
        r = null;
      }
      if (!r) {
        const holder = el.ownerDocument.createElement('div');
        r = { kind: row.kind, el: holder, ...field(holder, row) };
        rows.set(row.key, r);
      } else {
        r.update(row.kind === 'color' ? { ...row, color: row.value } : row);
      }
      list.append(r.el);
    }
    for (const [key, r] of rows) {
      if (seen.has(key)) continue;
      r.destroy();
      r.el.remove();
      rows.delete(key);
    }
  };
  render(props);
  return {
    update: render,
    destroy() {
      for (const r of rows.values()) r.destroy();
      rows.clear();
      list.replaceChildren();
    },
  };
}
