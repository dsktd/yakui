// A grid of colours to pick from.
// props: { colors: ["#rrggbb"], selected?: "#rrggbb", columns?: number }
// emits: pick { value } (the colour)
export function mount(el, props, emit) {
  const shadow = el.attachShadow({ mode: 'open' });
  shadow.innerHTML = `<style>
    .grid { display: grid; gap: 4px; padding: 4px;
            grid-template-columns: repeat(var(--columns, 6), 1fr); }
    .cell { aspect-ratio: 1; border: 2px solid transparent; border-radius: 3px; cursor: pointer; }
    .cell:hover { border-color: #aaa; }
    .cell.selected { border-color: #fff; }
  </style><div class="grid"></div>`;
  const grid = shadow.querySelector('.grid');
  const render = (props) => {
    grid.style.setProperty('--columns', props.columns ?? 6);
    grid.replaceChildren();
    for (const color of props.colors ?? []) {
      const cell = el.ownerDocument.createElement('div');
      cell.className = color === props.selected ? 'cell selected' : 'cell';
      cell.style.background = color;
      cell.title = color;
      cell.onclick = () => emit('pick', { value: color });
      grid.append(cell);
    }
  };
  render(props);
  return { update: render, destroy: () => grid.replaceChildren() };
}
