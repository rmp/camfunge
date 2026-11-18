const clamp01 = v => Math.min(1, Math.max(0, v));
const PIXEL_NOISE = 0.03;

/**
 * Analyses a downsampled RGBA frame against the previous frame's luma.
 * Scores are 0..1; focus is the motion centroid in video coordinates (0..1, y down) or null;
 * zones are mean per-pixel motion over a 3×3 grid, row-major from the video's top-left.
 */
export function analyseFrame(rgba, width, prevLuma) {
  const n = rgba.length / 4, height = n / width, luma = new Float32Array(n);
  const zones = new Float32Array(9), zoneCounts = new Float32Array(9);
  let motion = 0, chroma = 0, mass = 0, mx = 0, my = 0;
  for (let i = 0; i < n; i++) {
    const r = rgba[i * 4] / 255, g = rgba[i * 4 + 1] / 255, b = rgba[i * 4 + 2] / 255;
    luma[i] = 0.299 * r + 0.587 * g + 0.114 * b;
    chroma += Math.max(r, g, b) - Math.min(r, g, b);
    const diff = prevLuma ? Math.abs(luma[i] - prevLuma[i]) : 0;
    motion += diff;
    const x = i % width, y = Math.floor(i / width), moved = Math.max(0, diff - PIXEL_NOISE);
    mass += moved;
    mx += moved * (x + 0.5);
    my += moved * (y + 0.5);
    const zone = Math.floor((3 * y) / height) * 3 + Math.floor((3 * x) / width);
    zones[zone] += moved;
    zoneCounts[zone]++;
  }
  for (let z = 0; z < 9; z++) zones[z] /= zoneCounts[z];
  // ponytail: fixed noise floors and gains tuned by eye; per-camera auto-calibration if they misbehave
  return {
    luma,
    movement: clamp01((motion / n - 0.01) * 12),
    colour: clamp01((chroma / n - 0.1) * 2.5),
    focus: mass > 0 ? [mx / mass / width, my / mass / height] : null,
    zones,
  };
}

/** Index of the one zone whose motion clearly dominates the rest, or -1. */
export function dominantZone(zones, threshold = 0.04) {
  let best = 0, total = 0;
  zones.forEach((v, i) => {
    total += v;
    if (v > zones[best]) best = i;
  });
  const others = (total - zones[best]) / (zones.length - 1);
  return zones[best] > threshold && zones[best] > 3 * others ? best : -1;
}

/** Scores a block of time-domain audio samples (-1..1) 0..1 by loudness. */
export function soundSignal(samples) {
  let sum = 0;
  for (const s of samples) sum += s * s;
  const rms = Math.sqrt(sum / samples.length);
  // ponytail: fixed noise floor and gain; mic-gain calibration if quiet rooms read hot
  return clamp01((rms - 0.01) * 8);
}
