//! BGRA → YUV conversions for virtual-camera drivers that want YUV (limited/"video" range).

/// RGB → Y'CbCr matrix rows, already scaled for 8-bit limited range (Y 16..235, C 16..240).
#[derive(Clone, Copy)]
pub struct Matrix {
    y: [f32; 3],
    u: [f32; 3],
    v: [f32; 3],
}

/// SD webcams and most YUYV consumers.
pub const BT601: Matrix = Matrix {
    y: [0.256788, 0.504129, 0.097906],
    u: [-0.148223, -0.290993, 0.439216],
    v: [0.439216, -0.367788, -0.071427],
};

/// HD video; what OBS's own virtual camera sends by default.
pub const BT709: Matrix = Matrix {
    y: [0.182586, 0.614231, 0.062007],
    u: [-0.100644, -0.338572, 0.439216],
    v: [0.439216, -0.398942, -0.040274],
};

#[inline]
fn dot(m: [f32; 3], [r, g, b]: [f32; 3]) -> f32 {
    m[0] * r + m[1] * g + m[2] * b
}

#[inline]
fn rgb(bgra: &[u8], i: usize) -> [f32; 3] {
    [bgra[i * 4 + 2] as f32, bgra[i * 4 + 1] as f32, bgra[i * 4] as f32]
}

#[inline]
fn byte(v: f32) -> u8 {
    (v + 0.5).clamp(0.0, 255.0) as u8
}

/// Packed 4:2:2 `Y0 U Y1 V` (V4L2 `YUYV`, a.k.a. YUY2). `width` must be even.
pub fn bgra_to_yuyv(bgra: &[u8], width: usize, m: Matrix, out: &mut Vec<u8>) {
    let pixels = bgra.len() / 4;
    out.clear();
    out.reserve(pixels * 2);
    debug_assert_eq!(width % 2, 0);
    for pair in (0..pixels).step_by(2) {
        let (a, b) = (rgb(bgra, pair), rgb(bgra, pair + 1));
        let avg = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0];
        out.extend([byte(16.0 + dot(m.y, a)), byte(128.0 + dot(m.u, avg)), byte(16.0 + dot(m.y, b)), byte(128.0 + dot(m.v, avg))]);
    }
}

/// Planar Y then interleaved UV at quarter resolution (`NV12`). Width and height must be even.
pub fn bgra_to_nv12(bgra: &[u8], width: usize, height: usize, m: Matrix, out: &mut [u8]) {
    let (luma, chroma) = out.split_at_mut(width * height);
    for i in 0..width * height {
        luma[i] = byte(16.0 + dot(m.y, rgb(bgra, i)));
    }
    for cy in 0..height / 2 {
        for cx in 0..width / 2 {
            let mut sum = [0.0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let c = rgb(bgra, (cy * 2 + dy) * width + cx * 2 + dx);
                for k in 0..3 {
                    sum[k] += c[k] / 4.0;
                }
            }
            let o = cy * width + cx * 2;
            chroma[o] = byte(128.0 + dot(m.u, sum));
            chroma[o + 1] = byte(128.0 + dot(m.v, sum));
        }
    }
}

pub fn nv12_len(width: usize, height: usize) -> usize {
    width * height * 3 / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(b: u8, g: u8, r: u8, n: usize) -> Vec<u8> {
        [b, g, r, 255].repeat(n)
    }

    #[test]
    fn yuyv_black_white_and_red() {
        let mut out = Vec::new();
        bgra_to_yuyv(&solid(0, 0, 0, 2), 2, BT601, &mut out);
        assert_eq!(out, [16, 128, 16, 128]);
        bgra_to_yuyv(&solid(255, 255, 255, 2), 2, BT601, &mut out);
        assert_eq!(out, [235, 128, 235, 128]);
        bgra_to_yuyv(&solid(0, 0, 255, 2), 2, BT601, &mut out);
        assert_eq!(out, [81, 90, 81, 240]);
    }

    #[test]
    fn yuyv_shares_chroma_between_a_pixel_pair() {
        let mut out = Vec::new();
        let pair = [solid(0, 0, 0, 1), solid(255, 255, 255, 1)].concat();
        bgra_to_yuyv(&pair, 2, BT601, &mut out);
        assert_eq!(out, [16, 128, 235, 128]);
    }

    #[test]
    fn nv12_layout_and_bt709_red() {
        let (w, h) = (4, 2);
        let mut out = vec![0; nv12_len(w, h)];
        bgra_to_nv12(&solid(0, 0, 255, w * h), w, h, BT709, &mut out);
        assert_eq!(&out[..8], &[63; 8]);
        assert_eq!(&out[8..], &[102, 240, 102, 240]);
    }

    #[test]
    fn nv12_averages_each_2x2_block() {
        // Left 2×2 block: white; right: black. Chroma neutral, luma split.
        let (w, h) = (4, 2);
        let row = [solid(255, 255, 255, 2), solid(0, 0, 0, 2)].concat();
        let mut out = vec![0; nv12_len(w, h)];
        bgra_to_nv12(&row.repeat(2), w, h, BT709, &mut out);
        assert_eq!(&out[..4], &[235, 235, 16, 16]);
        assert_eq!(&out[8..], &[128, 128, 128, 128]);
    }
}
