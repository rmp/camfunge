/** Scores a downsampled RGBA frame 0..1 for movement (vs prevLuma) or colour vividness. */
export function sceneSignal(rgba, prevLuma, mode) {
  const n = rgba.length / 4, luma = new Float32Array(n);
  let motion = 0, chroma = 0;
  for (let i = 0; i < n; i++) {
    const r = rgba[i * 4] / 255, g = rgba[i * 4 + 1] / 255, b = rgba[i * 4 + 2] / 255;
    luma[i] = 0.299 * r + 0.587 * g + 0.114 * b;
    motion += prevLuma ? Math.abs(luma[i] - prevLuma[i]) : 0;
    chroma += Math.max(r, g, b) - Math.min(r, g, b);
  }
  // ponytail: fixed noise floors and gains tuned by eye; per-camera auto-calibration if they misbehave
  const raw = mode === 'colour' ? (chroma / n - 0.1) * 2.5 : (motion / n - 0.01) * 12;
  return { signal: Math.min(1, Math.max(0, raw)), luma };
}
