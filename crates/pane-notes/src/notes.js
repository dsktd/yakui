// Typing is local. The text goes to Rust as one SetText after a short pause;
// the Text event that comes back is applied only when nothing is pending.
export function mount(root, { send, onEvent }) {
  root.innerHTML = `<style>
    textarea { box-sizing: border-box; width: 100%; height: 100%; margin: 0; padding: 6px;
               border: 0; resize: none; outline: none; font: 14px monospace;
               background: #1e1e1e; color: #ddd; }
  </style><textarea spellcheck="false"></textarea>`;
  const ta = root.querySelector('textarea');
  let timer = null;
  let sent = null;
  const flush = () => {
    timer = null;
    sent = ta.value;
    send({ SetText: { text: sent } });
  };
  ta.addEventListener('input', () => {
    clearTimeout(timer);
    timer = setTimeout(flush, 200);
  });
  onEvent((event) => {
    const text = event.Text?.text;
    if (text === undefined) return;
    if (sent !== null && text !== sent) {
      // Not our echo yet (or it got lost): say it again.
      send({ SetText: { text: sent } });
      return;
    }
    sent = null;
    if (timer === null && ta.value !== text) ta.value = text;
  });
  return () => clearTimeout(timer);
}
