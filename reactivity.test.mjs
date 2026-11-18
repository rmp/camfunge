import { test } from 'node:test';
import assert from 'node:assert/strict';
import { analyseFrame, dominantZone, soundSignal } from './src/reactivity.js';

const W = 12, H = 12;
const frame = (r, g, b) => new Uint8ClampedArray(W * H * 4).map((_, i) => [r, g, b, 255][i % 4]);
const withBlock = (base, x0, y0, size) => {
  const f = base.slice();
  for (let y = y0; y < y0 + size; y++) for (let x = x0; x < x0 + size; x++) f.set([255, 255, 255], (y * W + x) * 4);
  return f;
};

test('movement: first frame and still scene are 0, big change is 1', () => {
  const grey = analyseFrame(frame(100, 100, 100), W, null);
  assert.equal(grey.movement, 0);
  assert.equal(grey.focus, null);
  assert.equal(analyseFrame(frame(100, 100, 100), W, grey.luma).movement, 0);
  assert.equal(analyseFrame(frame(250, 250, 250), W, grey.luma).movement, 1);
});

test('colour: grey is 0, saturated red is 1', () => {
  assert.equal(analyseFrame(frame(128, 128, 128), W, null).colour, 0);
  assert.equal(analyseFrame(frame(255, 0, 0), W, null).colour, 1);
});

test('focus and zones locate motion in the bottom-right', () => {
  const black = frame(0, 0, 0), before = analyseFrame(black, W, null);
  const after = analyseFrame(withBlock(black, 8, 8, 4), W, before.luma);
  after.focus.forEach(v => assert.ok(Math.abs(v - 10 / 12) < 1e-9));
  assert.equal(dominantZone(after.zones), 8);
});

test('camera noise below the pixel floor has no focus', () => {
  const a = analyseFrame(frame(100, 100, 100), W, null);
  assert.equal(analyseFrame(frame(104, 104, 104), W, a.luma).focus, null);
});

test('dominantZone: none when still or when motion is everywhere', () => {
  assert.equal(dominantZone(new Float32Array(9)), -1);
  assert.equal(dominantZone(new Float32Array(9).fill(0.3)), -1);
  assert.equal(dominantZone(Float32Array.from([0.3, 0, 0, 0, 0, 0, 0, 0, 0])), 0);
});

test('sound: silence is 0, full-scale tone is 1', () => {
  assert.equal(soundSignal(new Float32Array(256)), 0);
  assert.equal(soundSignal(new Float32Array(256).map((_, i) => Math.sin(i / 4))), 1);
});
