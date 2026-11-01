// The client side of PROTOCOL.md: the names a pane's web half and the widgets share.
// Topics and commands are the Rust half's business; a web half only needs the widget
// event names and the shapes its own Rust half hands it.

// What a widget may emit. The pane decides what, if anything, to do with it.
export const Emits = Object.freeze({
  SELECT: 'select', // { ids }
  HOVER: 'hover',   // { id } (null = none)
  CHANGE: 'change', // { key, value }, a committed field
  PICK: 'pick',     // { value }, a one-shot choice
  ACTION: 'action', // { name }, a button or tool
});

// Object kinds the gallery draws; anything else draws as a box.
export const Kind = Object.freeze({ BOX: 'box', SPHERE: 'sphere', LIGHT: 'light' });

export function isColor(value) {
  return typeof value === 'string' && /^#[0-9a-fA-F]{6}$/.test(value);
}
