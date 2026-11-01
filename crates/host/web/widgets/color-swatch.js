// A colour with its hex code; clicking it opens the browser's picker. The colour goes out
// when the picker commits, not while it is being dragged.
// props: { key, color, label? }
// emits: change { key, value } (value is "#rrggbb")
export function mount(el, props, emit) {
  const shadow = el.attachShadow({ mode: 'open' });
  shadow.innerHTML = `<style>
    label { display: flex; align-items: center; gap: 6px; font: 12px system-ui; color: #bbb;
            cursor: pointer; }
    .text { flex: 1; }
    .swatch { width: 18px; height: 18px; border: 1px solid #666; border-radius: 3px; }
    code { color: #999; }
    input { width: 0; height: 0; opacity: 0; border: 0; padding: 0; margin: 0; }
  </style><label><span class="text"></span><span class="swatch"></span><code></code>
  <input type="color"></label>`;
  const text = shadow.querySelector('.text');
  const swatch = shadow.querySelector('.swatch');
  const code = shadow.querySelector('code');
  const input = shadow.querySelector('input');
  let key = null;
  const render = (props) => {
    key = props.key ?? null;
    const color = /^#[0-9a-fA-F]{6}$/.test(props.color) ? props.color : '#000000';
    text.textContent = props.label ?? '';
    swatch.style.background = color;
    code.textContent = color;
    input.value = color;
  };
  const commit = () => emit('change', { key, value: input.value });
  input.addEventListener('change', commit);
  render(props);
  return { update: render, destroy: () => input.removeEventListener('change', commit) };
}
