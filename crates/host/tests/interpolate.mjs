// interpolate() from web/protocol.js: `node crates/host/tests/interpolate.mjs`.
import assert from 'node:assert/strict';
import { EXTRAPOLATE, interpolate } from '../web/protocol.js';

// Sender at 1 unit per ms along x, a sample every 10 ms, each arriving 10 ms later.
const stream = () => {
  const s = interpolate('drag/a');
  for (const ts of [0, 10, 20]) s.push([ts, 0, 0], ts, ts + 10);
  return s;
};

// One sample interval behind the newest: lerp between the samples around that time.
{
  const s = stream();
  assert.equal(s.topic, 'drag/a');
  assert.deepEqual(s.at(30), [10, 0, 0]);
  assert.deepEqual(s.at(25), [5, 0, 0]);
  assert.deepEqual(s.at(37), [17, 0, 0]);
  // A late arrival does not move the clock; an old or malformed sample is ignored.
  s.push([30, 0, 0], 30, 60);
  assert.deepEqual(s.at(45), [25, 0, 0]);
  s.push([99, 0, 0], 25, 61);
  s.push(['x', 0, 0], 40, 62);
  s.push([1, 2], 41, 63);
  assert.deepEqual(s.at(45), [25, 0, 0]);
}

// Samples stop: carried on along the last velocity, for EXTRAPOLATE ms and no further.
{
  const s = stream();
  assert.deepEqual(s.at(45), [25, 0, 0]);
  assert.deepEqual(s.at(1000), [20 + EXTRAPOLATE, 0, 0]);
  assert.deepEqual(s.at(5000), s.at(1000));
}

// The retained value arrives: the stream is forgotten, and a new one starts afresh.
{
  const s = stream();
  assert.ok(s.live);
  s.snap();
  assert.equal(s.at(30), null);
  assert.ok(!s.live);
  s.push([7, 0, 0], 500, 9000);
  assert.deepEqual(s.at(9000), [7, 0, 0]);
  s.push([8, 0, 0], 510, 9010);
  assert.deepEqual(s.at(9010), [7, 0, 0]);
}

console.log('interpolate: ok');
