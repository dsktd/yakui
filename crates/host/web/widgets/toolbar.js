// A row of buttons.
// props: { items: [{ name, label?, title?, active? }] }
// emits: action { name }
export function mount(el, props, emit) {
  const shadow = el.attachShadow({ mode: 'open' });
  shadow.innerHTML = `<style>
    .bar { display: flex; gap: 4px; padding: 3px; background: #2a2a2a; }
    button { font: inherit; background: #444; color: #ddd; border: 0; border-radius: 2px;
             padding: 1px 7px; cursor: pointer; }
    button:hover { background: #666; }
    button.active { background: #3a6ea5; }
  </style><div class="bar"></div>`;
  const bar = shadow.querySelector('.bar');
  const render = (props) => {
    bar.replaceChildren();
    for (const item of props.items ?? []) {
      const button = el.ownerDocument.createElement('button');
      button.textContent = item.label ?? item.name;
      if (item.title) button.title = item.title;
      button.classList.toggle('active', !!item.active);
      button.onclick = () => emit('action', { name: item.name });
      bar.append(button);
    }
  };
  render(props);
  return { update: render, destroy: () => bar.replaceChildren() };
}
