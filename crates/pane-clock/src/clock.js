export function mount(root) {
  root.innerHTML = `<style>
    .clock { display: flex; height: 100%; align-items: center; justify-content: center;
             font: bold 2.5em monospace; }
  </style><div class="clock"></div>`;
  const el = root.querySelector('.clock');
  const tick = () => { el.textContent = new Date().toLocaleTimeString(); };
  tick();
  const timer = setInterval(tick, 250);
  return () => clearInterval(timer);
}
