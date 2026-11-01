// One labelled number. Typing is local; the value goes out when committed (Enter, blur or
// the spinner), never per keystroke. A new value from props does not clobber a field being
// edited.
// props: { key, value, label?, min?, max?, step? }
// emits: change { key, value }
export function mount(el, props, emit) {
  const shadow = el.attachShadow({ mode: 'open' });
  shadow.innerHTML = `<style>
    label { display: flex; align-items: center; gap: 6px; font: 12px system-ui; color: #bbb; }
    span { flex: 1; }
    input { width: 7em; font: inherit; background: #1a1a1a; color: #eee; border: 1px solid #444;
            border-radius: 2px; padding: 2px 4px; }
  </style><label><span></span><input type="number"></label>`;
  const text = shadow.querySelector('span');
  const input = shadow.querySelector('input');
  let key = null;
  const render = (props) => {
    key = props.key ?? null;
    text.textContent = props.label ?? '';
    for (const attr of ['min', 'max', 'step']) {
      if (props[attr] === undefined) input.removeAttribute(attr);
      else input.setAttribute(attr, props[attr]);
    }
    if (shadow.activeElement !== input) input.value = props.value ?? '';
  };
  const commit = () => {
    const value = Number(input.value);
    if (Number.isFinite(value)) emit('change', { key, value });
  };
  const enter = (e) => { if (e.key === 'Enter') input.blur(); };
  input.addEventListener('change', commit);
  input.addEventListener('keydown', enter);
  render(props);
  return {
    update: render,
    destroy() {
      input.removeEventListener('change', commit);
      input.removeEventListener('keydown', enter);
    },
  };
}
