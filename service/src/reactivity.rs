//! Cheap CPU-side scene analysis that drives the filters' reactivity (ported from the original `reactivity.js`).

const PIXEL_NOISE: f32 = 0.03;

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

#[derive(Debug, Clone)]
pub struct Analysis {
    pub luma: Vec<f32>,
    /// 0..1, overall frame-to-frame change.
    pub movement: f32,
    /// 0..1, mean saturation.
    pub colour: f32,
    /// Motion centroid in video coordinates (0..1, y down), or `None` when nothing moved.
    pub focus: Option<[f32; 2]>,
    /// Mean per-pixel motion over a 3×3 grid, row-major from the video's top-left.
    pub zones: [f32; 9],
}

/// Analyses a downsampled RGBA frame against the previous frame's luma.
pub fn analyse_frame(rgba: &[u8], width: usize, prev_luma: Option<&[f32]>) -> Analysis {
    let n = rgba.len() / 4;
    let height = n / width;
    let mut luma = vec![0.0f32; n];
    let mut zones = [0.0f32; 9];
    let mut zone_counts = [0.0f32; 9];
    let (mut motion, mut chroma, mut mass, mut mx, mut my) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for i in 0..n {
        let [r, g, b] = [rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]].map(|c| c as f32 / 255.0);
        luma[i] = 0.299 * r + 0.587 * g + 0.114 * b;
        chroma += r.max(g).max(b) - r.min(g).min(b);
        let diff = prev_luma.map_or(0.0, |p| (luma[i] - p[i]).abs());
        motion += diff;
        let (x, y) = (i % width, i / width);
        let moved = (diff - PIXEL_NOISE).max(0.0);
        mass += moved;
        mx += moved * (x as f32 + 0.5);
        my += moved * (y as f32 + 0.5);
        let zone = (3 * y / height) * 3 + (3 * x / width);
        zones[zone] += moved;
        zone_counts[zone] += 1.0;
    }
    for z in 0..9 {
        zones[z] /= zone_counts[z].max(1.0);
    }
    let nf = n as f32;
    // ponytail: fixed noise floors and gains tuned by eye; per-camera auto-calibration if they misbehave
    Analysis {
        luma,
        movement: clamp01((motion / nf - 0.01) * 12.0),
        colour: clamp01((chroma / nf - 0.1) * 2.5),
        focus: (mass > 0.0).then(|| [mx / mass / width as f32, my / mass / height as f32]),
        zones,
    }
}

/// Index of the one zone whose motion clearly dominates the rest, or `None`.
pub fn dominant_zone(zones: &[f32; 9], threshold: f32) -> Option<usize> {
    let mut best = 0;
    for (i, &v) in zones.iter().enumerate() {
        if v > zones[best] {
            best = i;
        }
    }
    let total: f32 = zones.iter().sum();
    let others = (total - zones[best]) / (zones.len() - 1) as f32;
    (zones[best] > threshold && zones[best] > 3.0 * others).then_some(best)
}

pub const DEFAULT_ZONE_THRESHOLD: f32 = 0.04;

/// Scores a block of time-domain audio samples (-1..1) 0..1 by loudness.
pub fn sound_signal(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    // ponytail: fixed noise floor and gain; mic-gain calibration if quiet rooms read hot
    clamp01((rms - 0.01) * 8.0)
}

/// Box-filters an RGBA frame down to `w`×`h` (the analysis probe size).
pub fn downsample(rgba: &[u8], src_w: usize, src_h: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let (y0, y1) = (y * src_h / h, ((y + 1) * src_h / h).max(y * src_h / h + 1));
        for x in 0..w {
            let (x0, x1) = (x * src_w / w, ((x + 1) * src_w / w).max(x * src_w / w + 1));
            let mut sum = [0u32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = (sy * src_w + sx) * 4;
                    for c in 0..4 {
                        sum[c] += rgba[p + c] as u32;
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            for c in 0..4 {
                out[(y * w + x) * 4 + c] = (sum[c] / count) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 12;

    fn frame(r: u8, g: u8, b: u8) -> Vec<u8> {
        [r, g, b, 255].repeat(W * W)
    }

    fn with_block(base: &[u8], x0: usize, y0: usize, size: usize) -> Vec<u8> {
        let mut f = base.to_vec();
        for y in y0..y0 + size {
            for x in x0..x0 + size {
                f[(y * W + x) * 4..][..3].copy_from_slice(&[255, 255, 255]);
            }
        }
        f
    }

    #[test]
    fn movement_first_frame_and_still_scene_are_0_big_change_is_1() {
        let grey = analyse_frame(&frame(100, 100, 100), W, None);
        assert_eq!(grey.movement, 0.0);
        assert_eq!(grey.focus, None);
        assert_eq!(analyse_frame(&frame(100, 100, 100), W, Some(&grey.luma)).movement, 0.0);
        assert_eq!(analyse_frame(&frame(250, 250, 250), W, Some(&grey.luma)).movement, 1.0);
    }

    #[test]
    fn colour_grey_is_0_saturated_red_is_1() {
        assert_eq!(analyse_frame(&frame(128, 128, 128), W, None).colour, 0.0);
        assert_eq!(analyse_frame(&frame(255, 0, 0), W, None).colour, 1.0);
    }

    #[test]
    fn focus_and_zones_locate_motion_in_the_bottom_right() {
        let black = frame(0, 0, 0);
        let before = analyse_frame(&black, W, None);
        let after = analyse_frame(&with_block(&black, 8, 8, 4), W, Some(&before.luma));
        for v in after.focus.unwrap() {
            assert!((v - 10.0 / 12.0).abs() < 1e-6);
        }
        assert_eq!(dominant_zone(&after.zones, DEFAULT_ZONE_THRESHOLD), Some(8));
    }

    #[test]
    fn camera_noise_below_the_pixel_floor_has_no_focus() {
        let a = analyse_frame(&frame(100, 100, 100), W, None);
        assert_eq!(analyse_frame(&frame(104, 104, 104), W, Some(&a.luma)).focus, None);
    }

    #[test]
    fn dominant_zone_none_when_still_or_when_motion_is_everywhere() {
        assert_eq!(dominant_zone(&[0.0; 9], DEFAULT_ZONE_THRESHOLD), None);
        assert_eq!(dominant_zone(&[0.3; 9], DEFAULT_ZONE_THRESHOLD), None);
        let mut one = [0.0; 9];
        one[0] = 0.3;
        assert_eq!(dominant_zone(&one, DEFAULT_ZONE_THRESHOLD), Some(0));
    }

    #[test]
    fn sound_silence_is_0_full_scale_tone_is_1() {
        assert_eq!(sound_signal(&[0.0; 256]), 0.0);
        let tone: Vec<f32> = (0..256).map(|i| (i as f32 / 4.0).sin()).collect();
        assert_eq!(sound_signal(&tone), 1.0);
        assert_eq!(sound_signal(&[]), 0.0);
    }

    #[test]
    fn downsample_averages_blocks() {
        // 4×2 frame: left half black, right half white → 2×1 probe.
        let mut src = Vec::new();
        for _ in 0..2 {
            src.extend([0, 0, 0, 255].repeat(2));
            src.extend([255, 255, 255, 255].repeat(2));
        }
        assert_eq!(downsample(&src, 4, 2, 2, 1), vec![0, 0, 0, 255, 255, 255, 255, 255]);
        // Upsampling still works (each output pixel reads at least one source pixel).
        assert_eq!(downsample(&[9, 8, 7, 6], 1, 1, 2, 2), [9, 8, 7, 6].repeat(4));
    }
}
