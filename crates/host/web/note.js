// A brief status message in a pane's note line, shared by the pane modules (widgets do not
// import it). While it shows, `el` has the class `warn`; afterwards it holds `after()`.
const timers = new WeakMap();

export function flash(el, text, after = () => '') {
  el.textContent = text;
  el.classList.add('warn');
  clearTimeout(timers.get(el));
  timers.set(el, setTimeout(() => {
    el.textContent = after();
    el.classList.remove('warn');
  }, 2500));
}
