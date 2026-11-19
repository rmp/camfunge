//! Turns the WebGL-era filter bodies in [`crate::filters`] into GLSL 4.50 that wgpu's naga frontend compiles
//! on every backend (Metal, Vulkan, DX12, GL).
//!
//! Coordinate convention: `vUv` is (0,0) at the bottom-left of the output, as it was under WebGL. wgpu textures
//! have row 0 at the top, so every lookup flips v once in [`PRELUDE`]; filter bodies need no changes.

use crate::filters::Filter;

/// Bytes in the uniform block; see [`crate::renderer::Uniforms::to_bytes`].
pub const UNIFORM_BYTES: u64 = 48;

/// Binding slots shared by every fragment program (one bind-group layout for all passes).
pub mod binding {
    pub const UNIFORMS: u32 = 0;
    pub const SAMPLER: u32 = 1;
    pub const CAM: u32 = 2;
    pub const PREV: u32 = 3;
    pub const PREV_CAM: u32 = 4;
    pub const MOTION: u32 = 5;
    pub const BG: u32 = 6;
    pub const HISTORY: u32 = 7;
    pub const TEXTURES: [u32; 6] = [CAM, PREV, PREV_CAM, MOTION, BG, HISTORY];
}

/// Number of frames kept by `past()`, laid out as an 8×8 atlas of `TILE_W`×`TILE_H` tiles.
pub const HISTORY_FRAMES: u32 = 64;
pub const TILE_W: u32 = 320;
pub const TILE_H: u32 = 180;

pub const PRELUDE: &str = r#"#version 450
layout(set = 0, binding = 0) uniform Uniforms {
  vec2 uRes;
  vec2 uCamScale;
  vec2 uFocus;
  float uTime;
  float uAmt;
  float uClock;
  float uBurst;
  float uHead;
  float uMirror;
};
layout(set = 0, binding = 1) uniform sampler uSampler;
layout(set = 0, binding = 2) uniform texture2D uCam;
layout(set = 0, binding = 3) uniform texture2D uPrev;
layout(set = 0, binding = 4) uniform texture2D uPrevCam;
layout(set = 0, binding = 5) uniform texture2D uMotion;
layout(set = 0, binding = 6) uniform texture2D uBg;
layout(set = 0, binding = 7) uniform texture2D uHistory;
layout(location = 0) in vec2 vUv;
layout(location = 0) out vec4 fragColor;

vec2 flipV(vec2 uv) { return vec2(uv.x, 1.0 - uv.y); }
vec2 camUv(vec2 uv) { uv = (uv - 0.5) * uCamScale + 0.5; return vec2(uMirror > 0.5 ? 1.0 - uv.x : uv.x, uv.y); }
vec3 cam(vec2 uv) { return texture(sampler2D(uCam, uSampler), flipV(camUv(uv))).rgb; }
vec3 prevCam(vec2 uv) { return texture(sampler2D(uPrevCam, uSampler), flipV(camUv(uv))).rgb; }
vec3 background(vec2 uv) { return texture(sampler2D(uBg, uSampler), flipV(camUv(uv))).rgb; }
vec4 prev4(vec2 uv) { return texture(sampler2D(uPrev, uSampler), flipV(uv)); }
vec3 prev(vec2 uv) { return prev4(uv).rgb; }
/** r: recent-motion heat (fades), gb: flow in uv per camera frame, a: motion this frame. */
vec4 motion(vec2 uv) { return texture(sampler2D(uMotion, uSampler), flipV(uv)); }
/** The output as it was framesAgo camera frames ago (0..63), at 320×180. */
vec3 past(vec2 uv, float framesAgo) {
  float tile = mod(uHead - framesAgo + 64.0, 64.0);
  vec2 inset = 0.5 / vec2(320.0, 180.0);
  return texture(sampler2D(uHistory, uSampler),
                 flipV((vec2(mod(tile, 8.0), floor(tile / 8.0)) + clamp(uv, inset, 1.0 - inset)) / 8.0)).rgb;
}
float luma(vec3 c) { return dot(c, vec3(0.299, 0.587, 0.114)); }
float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
mat2 rot(float a) { return mat2(cos(a), -sin(a), sin(a), cos(a)); }
vec3 rgb2hsv(vec3 c) {
  vec4 K = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
  vec4 p = mix(vec4(c.bg, K.wz), vec4(c.gb, K.xy), step(c.b, c.g));
  vec4 q = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
  float d = q.x - min(q.w, q.y);
  return vec3(abs(q.z + (q.w - q.y) / (6.0 * d + 1e-10)), d / (q.x + 1e-10), q.x);
}
vec3 hsv2rgb(vec3 c) { vec3 p = abs(fract(c.xxx + vec3(1.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0); return c.z * mix(vec3(1.0), clamp(p - 1.0, 0.0, 1.0), c.y); }
float halftone(vec2 uv, float angle, float cells, float ink) {
  vec2 f = fract(rot(angle) * uv * cells) - 0.5;
  return 1.0 - smoothstep(-0.05, 0.05, length(f) - sqrt(ink) * 0.55);
}
vec3 palette(float t) { return 0.5 + 0.5 * cos(6.2831 * (t + vec3(0.0, 0.33, 0.67))); }
vec2 grad(vec2 uv) {
  vec2 e = 1.5 / uRes;
  return vec2(luma(cam(uv + vec2(e.x, 0.0))) - luma(cam(uv - vec2(e.x, 0.0))),
              luma(cam(uv + vec2(0.0, e.y))) - luma(cam(uv - vec2(0.0, e.y))));
}
"#;

/// Full-screen triangle; `vUv` spans 0..1 with y up, matching the WebGL original.
pub const VERTEX: &str = r#"
struct Out { @builtin(position) pos: vec4f, @location(0) uv: vec2f };
@vertex fn main(@builtin(vertex_index) i: u32) -> Out {
  let p = vec2f(select(-1.0, 3.0, i == 1u), select(-1.0, 3.0, i == 2u));
  return Out(vec4f(p, 0.0, 1.0), p * 0.5 + 0.5);
}
"#;

/// Copies the previous-frame texture straight out (used for the final BGRA output).
pub const BLIT: &str = "gl_FragColor = vec4(prev(vUv), 1.0);";

/// A complete fragment shader for one filter body.
pub fn fragment(body: &str) -> String {
    format!("{PRELUDE}void main() {{{}}}\n", body.replace("gl_FragColor", "fragColor"))
}

pub fn filter_fragment(f: &Filter) -> String {
    fragment(f.src)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::{COPY, FILTERS, MOTION};

    fn validate(name: &str, src: &str, stage: naga::ShaderStage) {
        let module = match stage {
            naga::ShaderStage::Vertex if !src.contains("#version") => naga::front::wgsl::parse_str(src).map_err(|e| e.emit_to_string(src)),
            _ => naga::front::glsl::Frontend::default()
                .parse(&naga::front::glsl::Options::from(stage), src)
                .map_err(|e| e.emit_to_string(src)),
        }
        .unwrap_or_else(|e| panic!("{name} does not parse:\n{e}"));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name} does not validate: {e:?}"));
    }

    #[test]
    fn every_filter_compiles() {
        for f in FILTERS {
            validate(f.name, &filter_fragment(f), naga::ShaderStage::Fragment);
        }
    }

    #[test]
    fn internal_passes_compile() {
        validate("motion", &fragment(MOTION), naga::ShaderStage::Fragment);
        validate("copy", &fragment(COPY), naga::ShaderStage::Fragment);
        validate("blit", &fragment(BLIT), naga::ShaderStage::Fragment);
        validate("vertex", VERTEX, naga::ShaderStage::Vertex);
    }

    #[test]
    fn filter_names_are_unique() {
        let mut names: Vec<_> = FILTERS.iter().map(|f| f.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), FILTERS.len());
    }
}
