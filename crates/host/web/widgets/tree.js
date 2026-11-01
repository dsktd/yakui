// A tree of labelled nodes. Which branches are open is the only state of its own.
// props: { nodes: [{ id, label, children?: [node] }], selected?: [id], hovered?: id }
// emits: select { ids: [id] } on click, pick { value: id } on double click,
//        hover { id } on enter, hover { id: null } when the pointer leaves the tree
export function mount(el, props, emit) {
  const shadow = el.attachShadow({ mode: 'open' });
  shadow.innerHTML = `<style>
    .tree { font: 12px system-ui; color: #ccc; padding: 4px; user-select: none; }
    .row { display: flex; align-items: center; gap: 4px; padding: 2px 4px; border-radius: 2px;
           cursor: pointer; }
    .row:hover { background: #2e2e2e; }
    .row.hovered { background: #333; }
    .row.selected { background: #3a6ea5; color: #fff; }
    .caret { width: 1em; text-align: center; color: #888; }
    .children { margin-left: 1em; }
    .closed > .children { display: none; }
  </style><div class="tree"></div>`;
  const tree = shadow.querySelector('.tree');
  const closed = new Set();
  const leave = () => emit('hover', { id: null });
  const render = (props) => {
    const selected = new Set(props.selected ?? []);
    const node = (n) => {
      const doc = el.ownerDocument;
      const item = doc.createElement('div');
      const row = doc.createElement('div');
      row.className = 'row';
      if (selected.has(n.id)) row.classList.add('selected');
      if (n.id === props.hovered) row.classList.add('hovered');
      const caret = doc.createElement('span');
      caret.className = 'caret';
      const kids = n.children ?? [];
      caret.textContent = kids.length ? (closed.has(n.id) ? '▸' : '▾') : '';
      caret.onclick = (e) => {
        e.stopPropagation();
        if (closed.has(n.id)) closed.delete(n.id); else closed.add(n.id);
        item.classList.toggle('closed', closed.has(n.id));
        caret.textContent = closed.has(n.id) ? '▸' : '▾';
      };
      const label = doc.createElement('span');
      label.textContent = n.label ?? n.id;
      row.append(caret, label);
      row.onclick = () => emit('select', { ids: [n.id] });
      row.ondblclick = () => emit('pick', { value: n.id });
      row.onpointerenter = () => emit('hover', { id: n.id });
      item.append(row);
      if (kids.length) {
        const children = doc.createElement('div');
        children.className = 'children';
        for (const kid of kids) children.append(node(kid));
        item.append(children);
        item.classList.toggle('closed', closed.has(n.id));
      }
      return item;
    };
    tree.replaceChildren(...(props.nodes ?? []).map(node));
  };
  tree.addEventListener('pointerleave', leave);
  render(props);
  return {
    update: render,
    destroy() {
      tree.removeEventListener('pointerleave', leave);
      tree.replaceChildren();
    },
  };
}
