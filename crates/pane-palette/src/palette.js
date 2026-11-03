// A palette grid; a pick is one message. Which swatch is lit comes from the scene.
import { mount as paletteGrid } from 'yakui/widgets/palette-grid.js';
import { Emits } from 'yakui/protocol.js';
import { flash } from 'yakui/note.js';

export function mount(root, { send, onEvent }) {
  root.innerHTML = `<style>
    .note { font: 11px system-ui; color: #999; padding: 2px 6px; }
    .note.warn { color: #e6a700; }
  </style><div class="grid"></div><div class="note">select something first</div>`;
  const note = root.querySelector('.note');
  let status = '';
  const grid = paletteGrid(root.querySelector('.grid'), { colors: [], selected: null }, (name, { value }) => {
    if (name === Emits.PICK) send({ Pick: { color: value } });
  });
  onEvent((e) => {
    if (e.State) {
      const { colors, selected, color } = e.State;
      grid.update({ colors, selected: color ?? null, columns: 6 });
      status = selected.length ? `${selected.length} selected` : 'select something first';
      if (!note.classList.contains('warn')) note.textContent = status;
    } else if (e.Rejected) flash(note, e.Rejected.reason, () => status);
    else if (e.Unsupported) flash(note, `protocol version ${JSON.stringify(e.Unsupported.version)} is not one this pane knows`, () => status);
  });
  return () => grid.destroy();
}
