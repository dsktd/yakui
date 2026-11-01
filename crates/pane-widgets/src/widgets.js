// Every widget with sample props, and a log of what each emits. Where it makes sense the
// emitted value is fed back through update(), so the widget shows the round trip.
import { mount as toolbar } from '../widgets/toolbar.js';
import { mount as propertyList } from '../widgets/property-list.js';
import { mount as numberField } from '../widgets/number-field.js';
import { mount as colorSwatch } from '../widgets/color-swatch.js';
import { mount as paletteGrid } from '../widgets/palette-grid.js';
import { mount as tree } from '../widgets/tree.js';
import { Emits } from '../protocol.js';

export function mount(root) {
  root.innerHTML = `<style>
    .gallery { display: flex; flex-direction: column; height: 100%; font: 12px system-ui; color: #ccc; }
    .cards { flex: 1; overflow: auto; padding: 6px; display: grid; gap: 8px;
             grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); align-content: start; }
    .card { background: #242424; border-radius: 4px; padding: 6px; }
    .card h3 { margin: 0 0 6px; font: bold 12px system-ui; color: #999; }
    .log { flex: none; max-height: 30%; overflow: auto; border-top: 1px solid #333;
           padding: 4px 6px; font: 11px monospace; color: #9c9; }
  </style><div class="gallery"><div class="cards"></div><div class="log"></div></div>`;
  const cards = root.querySelector('.cards');
  const log = root.querySelector('.log');
  const widgets = [];
  const say = (widget, name, payload) => {
    const line = root.ownerDocument.createElement('div');
    line.textContent = `${widget}: ${name} ${JSON.stringify(payload)}`;
    log.prepend(line);
    while (log.childElementCount > 50) log.lastChild.remove();
  };
  // A card holds one widget; `react` gets (name, payload, update, current) to feed a value
  // back; update merges into the widget's current props, not the first ones.
  const card = (title, mountFn, props, react) => {
    const el = root.ownerDocument.createElement('div');
    el.className = 'card';
    el.innerHTML = '<h3></h3><div></div>';
    el.querySelector('h3').textContent = title;
    cards.append(el);
    let current = props;
    const widget = mountFn(el.lastChild, props, (name, payload) => {
      say(title, name, payload);
      react?.(name, payload, (next) => widget.update((current = { ...current, ...next })), current);
    });
    widgets.push(widget);
  };

  card('toolbar', toolbar, {
    items: [
      { name: 'select', label: 'select', active: true },
      { name: 'move', label: 'move' },
      { name: 'rotate', label: 'rotate', title: 'not really' },
    ],
  }, (name, { name: pressed }, update) => {
    if (name !== Emits.ACTION) return;
    update({ items: ['select', 'move', 'rotate'].map((n) => ({ name: n, label: n, active: n === pressed })) });
  });

  const rows = [
    { key: 'name', label: 'name', kind: 'text', value: 'Box' },
    { key: 'weight', label: 'weight', kind: 'number', value: 2.5, step: 0.5 },
    { key: 'color', label: 'colour', kind: 'color', value: '#3a8ee0' },
  ];
  card('property-list', propertyList, { rows }, (name, { key, value }, update, current) => {
    if (name !== Emits.CHANGE) return;
    update({ rows: current.rows.map((r) => (r.key === key ? { ...r, value } : r)) });
  });

  card('number-field', numberField, { key: 'angle', label: 'angle', value: 45, min: 0, max: 360, step: 5 },
    (name, { value }, update) => { if (name === Emits.CHANGE) update({ value }); });

  card('color-swatch', colorSwatch, { key: 'color', label: 'colour', color: '#e06c3a' },
    (name, { value }, update) => { if (name === Emits.CHANGE) update({ color: value }); });

  card('palette-grid', paletteGrid, {
    colors: ['#e06c3a', '#e0b03a', '#a8d03a', '#3ad06c', '#3ac0c0', '#3a8ee0', '#6c3ae0', '#c03ae0', '#e03a8e', '#ffffff', '#8a8a8a', '#222222'],
    selected: '#3a8ee0',
    columns: 6,
  }, (name, { value }, update) => { if (name === Emits.PICK) update({ selected: value }); });

  card('tree', tree, {
    nodes: [
      { id: 'kind:box', label: 'box (2)', children: [{ id: 'a', label: 'Box' }, { id: 'b', label: 'Slab' }] },
      { id: 'kind:sphere', label: 'sphere (1)', children: [{ id: 'c', label: 'Ball' }] },
      { id: 'kind:light', label: 'light (1)', children: [{ id: 'd', label: 'Lamp' }] },
    ],
    selected: ['a'],
  }, (name, payload, update) => {
    if (name === Emits.SELECT) update({ selected: payload.ids });
    if (name === Emits.HOVER) update({ hovered: payload.id });
  });

  return () => { for (const w of widgets) w.destroy(); };
}
