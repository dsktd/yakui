// A palette grid; a pick is one message. Which swatch is lit comes from the scene.
import { mount as paletteGrid } from '../widgets/palette-grid.js';
import { Emits } from '../protocol.js';

export function mount(root, { send, onEvent }) {
  root.innerHTML = `<style>
    .note { font: 11px system-ui; color: #999; padding: 2px 6px; }
    .note.warn { color: #e6a700; }
  </style><div class="grid"></div><div class="note">select something first</div>`;
  const note = root.querySelector('.note');
  let noteTimer = null;
  let status = '';
  const show = (text, warn) => {
    note.textContent = text;
    note.classList.toggle('warn', !!warn);
  };
  const flash = (text) => {
    show(text, true);
    clearTimeout(noteTimer);
    noteTimer = setTimeout(() => show(status, false), 2500);
  };
  const grid = paletteGrid(root.querySelector('.grid'), { colors: [], selected: null }, (name, { value }) => {
    if (name === Emits.PICK) send({ Pick: { color: value } });
  });
  onEvent((e) => {
    if (e.State) {
      const { colors, selected, color } = e.State;
      grid.update({ colors, selected: color ?? null, columns: 6 });
      status = selected.length ? `${selected.length} selected` : 'select something first';
      if (noteTimer === null) show(status, false);
    } else if (e.Rejected) flash(e.Rejected.reason);
    else if (e.Unsupported) flash(`protocol version ${JSON.stringify(e.Unsupported.version)} is not one this pane knows`);
  });
  return () => {
    grid.destroy();
    clearTimeout(noteTimer);
  };
}
