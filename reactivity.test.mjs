import { test } from 'node:test';
import assert from 'node:assert/strict';
import { sceneSignal, soundSignal } from './src/reactivity.js';

const frame = (r, g, b) => new Uint8ClampedArray(16 * 4).map((_, i) => [r, g, b, 255][i % 4]);

test('movement: still scene is 0, big change is 1, first frame is 0', () => {
  const grey = sceneSignal(frame(100, 100, 100), null, 'movement');
  assert.equal(grey.signal, 0);
  assert.equal(sceneSignal(frame(100, 100, 100), grey.luma, 'movement').signal, 0);
  assert.equal(sceneSignal(frame(250, 250, 250), grey.luma, 'movement').signal, 1);
});

test('colour: grey is 0, saturated red is 1', () => {
  assert.equal(sceneSignal(frame(128, 128, 128), null, 'colour').signal, 0);
  assert.equal(sceneSignal(frame(255, 0, 0), null, 'colour').signal, 1);
});

test('sound: silence is 0, full-scale tone is 1', () => {
  assert.equal(soundSignal(new Float32Array(256)), 0);
  assert.equal(soundSignal(new Float32Array(256).map((_, i) => Math.sin(i / 4))), 1);
});
