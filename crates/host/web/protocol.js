// The client side of PROTOCOL.md: the names a pane's web half and the widgets share.
// Topics and commands are the Rust half's business; a web half only needs the widget
// event names, the shapes its own Rust half hands it, and how to draw a stream of them.

// The protocol version, which is also the `lib/v<VERSION>/` this file and the widgets are
// served under. The host fills it in from `pane_api::protocol::VERSION`; bumping that is how
// a widget's props or emits change meaning (PROTOCOL.md "Shared web modules").
export const VERSION = /*@VERSION*/ 0;

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

// How far past its newest sample a stream is carried on when samples stop, in ms.
export const EXTRAPOLATE = 100;
// Samples kept per stream, and the most a stream is drawn behind its newest one, in ms.
const SAMPLES = 3;
const MAX_DELAY = 100;

const isVec =(v) => Array.isArray(v) && v.length === 3 && v.every(Number.isFinite);
export const lerp = (a, b, k) => a.map((x, i) => x + (b[i] - x) * k);

// One remote object's position from a transient stream (`topic`, e.g. `drag/<id>`), drawn
// smoothly: `push` each sample with its sender's `ts` (monotonic ms) as it arrives; `at`
// gives the position to draw now, one sample interval behind the newest, so there is
// always a pair to lerp between, and carried on along the last velocity for at most
// EXTRAPOLATE ms when samples stop. `snap` forgets the stream when the retained value
// arrives; `at` is then null and the caller draws the retained value. `now` is the
// receiver's monotonic ms; the two clocks are tied by the smallest delay seen. `ts` counts
// only within one sender's host, so a sample from another `origin` (the object passed to a
// pane there) starts the stream afresh; without `origin` the last one is kept.
export function interpolate(topic) {
  let samples = [];
  let offset = Infinity;
  let source;
  return {
    topic,
    push(position, ts, now = performance.now(), origin = source) {
      if (!isVec(position) || !Number.isFinite(ts)) return;
      if (origin !== source) {
        samples = [];
        offset = Infinity;
        source = origin;
      }
      if (samples.length && ts <= samples.at(-1).ts) return; // late or repeated
      offset = Math.min(offset, now - ts);
      samples.push({ position, ts });
      if (samples.length > SAMPLES) samples.shift();
    },
    at(now = performance.now()) {
      if (!samples.length) return null;
      const last = samples.at(-1);
      if (samples.length === 1) return last.position;
      const prev = samples.at(-2);
      const t = now - offset - Math.min(last.ts - prev.ts, MAX_DELAY);
      if (t <= samples[0].ts) return samples[0].position;
      for (let i = 1; i < samples.length; i++) {
        const [a, b] = [samples[i - 1], samples[i]];
        if (t <= b.ts) return lerp(a.position, b.position, (t - a.ts) / (b.ts - a.ts));
      }
      const ahead = Math.min(t - last.ts, EXTRAPOLATE);
      return lerp(prev.position, last.position, 1 + ahead / (last.ts - prev.ts));
    },
    snap() {
      samples = [];
      offset = Infinity;
    },
    get live() {
      return samples.length > 0;
    },
    get origin() {
      return source;
    },
  };
}
