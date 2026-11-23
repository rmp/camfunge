//! The trips: GLSL fragment-shader bodies run over the camera feed.
//! Each body sees the helpers in `shaders::PRELUDE` and writes `gl_FragColor`.

pub struct Filter {
    pub name: &'static str,
    pub artist: &'static str,
    pub src: &'static str,
}

/// Copies the (cover-scaled, optionally mirrored) camera.
pub const COPY: &str = r#"gl_FragColor = vec4(cam(vUv), 1.0);"#;

/// Motion pass. r: recent-motion heat (fades), gb: flow in uv per camera frame, a: motion this frame.
pub const MOTION: &str = r#"
  vec2 e = 4.0 / uRes;
  vec2 g = vec2(luma(cam(vUv + vec2(e.x, 0.0))) - luma(cam(vUv - vec2(e.x, 0.0))),
                luma(cam(vUv + vec2(0.0, e.y))) - luma(cam(vUv - vec2(0.0, e.y))));
  float change = luma(cam(vUv)) - luma(prevCam(vUv));
  float moved = smoothstep(0.03, 0.15, abs(change));
  vec2 flow = clamp(-change * g / (dot(g, g) + 0.004), -1.0, 1.0) * 2.0 * e * moved;
  vec4 was = prev4(vUv);
  gl_FragColor = vec4(max(moved, was.r * 0.96), mix(was.gb * 0.85, flow, moved), moved);"#;

pub const FILTERS: &[Filter] = &[
    Filter {
        name: "Straight",
        artist: "no filter",
        src: COPY,
    },
    Filter {
        name: "Lucy in the Sky",
        artist: "The Beatles",
        src: r#"
    vec2 aspect = vec2(uRes.x / uRes.y, 1.0);
    vec2 p = (vUv - uFocus) * aspect;
    float r = length(p), seg = 6.2831 / (6.0 + floor(uAmt * 8.0));
    float a = abs(mod(atan(p.y, p.x) + uTime * 0.2, seg) - seg * 0.5);
    vec2 q = vec2(cos(a), sin(a)) * r * (1.0 + 0.2 * sin(uTime * 0.7));
    q += 0.04 * uAmt * vec2(sin(r * 14.0 - uTime * 2.0), cos(r * 11.0 + uTime));
    vec3 h = rgb2hsv(cam(q / aspect + uFocus));
    h.x = fract(h.x + r * uAmt * 1.5 + uTime * 0.08);
    h.y = min(1.0, h.y * 1.8 + 0.4 * uAmt);
    gl_FragColor = vec4(hsv2rgb(h), 1.0);"#,
    },
    Filter {
        name: "Magical Mystery Tour",
        artist: "The Beatles",
        src: r#"
    float n = 2.0 + floor(uAmt * 2.99);
    vec2 cell = floor(vUv * n);
    vec2 uv = (fract(vUv * n) - 0.5) * (1.0 + 0.12 * sin(uTime * 1.5 + cell.x * 1.7 + cell.y * 2.3)) + 0.5;
    float l = floor(luma(cam(uv)) * 4.0) / 3.0;
    float hue = fract((cell.x + cell.y * n) / (n * n) + uTime * 0.05);
    vec3 ink = hsv2rgb(vec3(hue, 0.9, 1.0)), paper = hsv2rgb(vec3(fract(hue + 0.5), 0.8, 0.35));
    gl_FragColor = vec4(mix(paper, ink, l), 1.0);"#,
    },
    Filter {
        name: "Strawberry Fields",
        artist: "The Beatles",
        src: r#"
    float drip = (0.5 + 0.5 * sin(vUv.x * 21.0 + uTime)) * (0.5 + 0.5 * sin(vUv.x * 57.0 - uTime * 0.6));
    vec3 c = cam(vUv + vec2(0.0, drip * 0.18 * uAmt * vUv.y));
    vec3 h = rgb2hsv(c);
    h.x = fract(mix(h.x, h.x < 0.5 ? 0.95 : 0.35, 0.5 * uAmt) + 0.03 * sin(uTime));
    h.y = min(1.0, h.y + 0.5 * uAmt);
    c = hsv2rgb(h);
    vec3 melted = prev(vUv + vec2(0.0, 0.003 + 0.004 * drip)) * 0.94;
    gl_FragColor = vec4(mix(c, max(c, melted), 0.75), 1.0);"#,
    },
    Filter {
        name: "Helter Skelter",
        artist: "The Beatles",
        src: r#"
    vec2 aspect = vec2(uRes.x / uRes.y, 1.0);
    float zoom = 1.0 + 2.0 * uAmt * (0.6 + 0.4 * sin(uTime));
    vec2 uv = rot(uTime * 0.3) * (vUv - 0.5) * aspect * zoom / aspect + 0.5;
    vec3 h = rgb2hsv(cam(uv));
    h.x = fract(h.x + dot(floor(uv), vec2(0.25, 0.5)) + uTime * 0.05);
    h.y = min(1.0, h.y * 1.5 + 0.2);
    gl_FragColor = vec4(hsv2rgb(h), 1.0);"#,
    },
    Filter {
        name: "Sgt. Pepper",
        artist: "The Beatles",
        src: r#"
    vec2 uv = vUv * vec2(uRes.x / uRes.y, 1.0);
    float cells = mix(110.0, 35.0, uAmt) * (1.0 + 0.1 * sin(uTime));
    vec3 ink = 1.0 - cam(vUv);
    vec3 dots = vec3(halftone(uv, 0.26, cells, ink.r), halftone(uv, 1.31, cells, ink.g), halftone(uv, 0.79, cells, ink.b));
    vec3 paper = palette(uTime * 0.03) * 0.25 + 0.75;
    gl_FragColor = vec4(paper * (1.0 - dots), 1.0);"#,
    },
    Filter {
        name: "Across the Universe",
        artist: "The Beatles",
        src: r#"
    float heat = motion(vUv).r;
    vec3 ghost = hsv2rgb(vec3(fract(heat * 1.5 + uTime * 0.05), 1.0, smoothstep(0.02, 0.2, heat)));
    gl_FragColor = vec4(cam(vUv) * 0.4 + ghost * (0.5 + uAmt), 1.0);"#,
    },
    Filter {
        name: "Nowhere Man",
        artist: "The Beatles",
        src: r#"
    vec3 c = cam(vUv), bg = background(vUv);
    float subject = smoothstep(0.08, 0.25, length(c - bg));
    vec3 cloak = background(vUv + grad(vUv) * (0.02 + 0.08 * uAmt));
    gl_FragColor = vec4(mix(c, cloak, subject), 1.0);"#,
    },
    Filter {
        name: "Purple Haze",
        artist: "Jimi Hendrix",
        src: r#"
    vec2 w = vec2(sin(vUv.y * 8.0 + uTime * 1.3) + sin(vUv.y * 3.0 - uTime),
                  cos(vUv.x * 7.0 - uTime * 1.1) + cos(vUv.x * 2.0 + uTime * 0.7));
    vec3 c = cam(vUv + w * 0.025 * uAmt);
    float l = luma(c);
    vec3 haze = mix(vec3(0.22, 0.0, 0.45), vec3(1.0, 0.45, 0.08), smoothstep(0.15, 0.85, l + 0.2 * sin(uTime + l * 8.0)));
    haze = mix(c, haze, 0.8);
    vec3 trail = prev(vUv + w * 0.003 - (vUv - 0.5) * 0.008) * (0.8 + 0.15 * uAmt);
    gl_FragColor = vec4(max(haze, trail), 1.0);"#,
    },
    Filter {
        name: "Electric Ladyland",
        artist: "Jimi Hendrix",
        src: r#"
    vec2 g = grad(vUv);
    float e = smoothstep(0.03, 0.25, length(g));
    vec3 neon = hsv2rgb(vec3(fract(atan(g.y, g.x) / 6.2831 + uTime * 0.15), 1.0, 1.0)) * e * 1.6;
    vec3 c = cam(vUv);
    vec3 solar = abs(c - 0.5 + 0.5 * sin(uTime * 0.5)) * (1.0 - uAmt) * 0.6;
    gl_FragColor = vec4(max(neon + solar, prev(vUv) * (0.6 + 0.3 * uAmt)), 1.0);"#,
    },
    Filter {
        name: "Fire",
        artist: "Jimi Hendrix",
        src: r#"
    float heat = luma(cam(vUv)) + 0.12 * sin(uTime * 3.0 + vUv.y * 20.0);
    vec3 thermal = clamp(vec3(heat * 3.0, heat * 3.0 - 1.0, heat * 3.0 - 2.0), 0.0, 1.0);
    vec2 rise = vec2(0.003 * sin(vUv.y * 40.0 + uTime * 6.0), -0.004 - 0.006 * uAmt);
    vec3 flames = prev(vUv + rise) * vec3(0.97, 0.88, 0.7);
    gl_FragColor = vec4(max(thermal, flames * (0.6 + 0.35 * uAmt)), 1.0);"#,
    },
    Filter {
        name: "Voodoo Child",
        artist: "Jimi Hendrix",
        src: r#"
    vec3 c = cam(vUv), p = prev(vUv);
    vec3 o = vec3(c.r, mix(p.g, c.g, mix(0.5, 0.12, uAmt)), mix(p.b, c.b, mix(0.3, 0.05, uAmt)));
    gl_FragColor = vec4(mix(vec3(luma(o)), o, 1.7), 1.0);"#,
    },
    Filter {
        name: "Little Wing",
        artist: "Jimi Hendrix",
        src: r#"
    vec3 c = cam(vUv);
    vec3 hue = hsv2rgb(vec3(fract(uTime * 0.1 + vUv.x * 0.3), 0.9, 1.0));
    vec3 paint = mix(c, hue, 0.6) * smoothstep(0.1, 0.5, motion(vUv).a) * 1.5;
    gl_FragColor = vec4(max(c * 0.35, max(prev(vUv) * (0.9 + 0.08 * uAmt), paint)), 1.0);"#,
    },
    Filter {
        name: "Crosstown Traffic",
        artist: "Jimi Hendrix",
        src: r#"
    vec2 g = grad(vUv);
    float edge = smoothstep(0.03, 0.2, length(g)) * smoothstep(0.02, 0.3, motion(vUv).r);
    vec3 neon = hsv2rgb(vec3(fract(atan(g.y, g.x) / 6.2831 + uTime * 0.2), 1.0, 1.0)) * edge * 2.0;
    gl_FragColor = vec4(max(max(neon, prev(vUv) * (0.7 + 0.25 * uAmt)), cam(vUv) * 0.15), 1.0);"#,
    },
    Filter {
        name: "Piece of My Heart",
        artist: "Janis Joplin",
        src: r#"
    vec2 p = vUv - 0.5;
    float r = length(p);
    p = rot(uAmt * 4.0 * exp(-r * 4.0) * sin(uTime * 0.5)) * p;
    vec2 uv = p + 0.5;
    float l = luma(cam(uv));
    float band = floor(l * 5.0 + 0.6 * sin(uTime * 0.8 + r * 10.0));
    vec3 col = palette(band / 5.0 + uTime * 0.04);
    float ink = smoothstep(0.08, 0.2, length(grad(uv)));
    gl_FragColor = vec4(mix(col, vec3(0.08, 0.0, 0.1), ink * uAmt), 1.0);"#,
    },
    Filter {
        name: "Ball and Chain",
        artist: "Janis Joplin",
        src: r#"
    vec2 aspect = vec2(uRes.x / uRes.y, 1.0);
    vec2 p = (vUv - 0.5) * aspect;
    float r = length(p), a = atan(p.y, p.x);
    float petals = 0.06 * uAmt * sin(a * 6.0 + uTime * 1.5) * sin(r * 20.0 - uTime * 3.0);
    vec3 c = cam((p * (1.0 + petals)) / aspect + 0.5);
    float rings = 0.5 + 0.5 * sin(r * 40.0 - uTime * 4.0 + luma(c) * 6.0);
    vec3 paisley = palette(luma(c) + r + uTime * 0.07);
    gl_FragColor = vec4(mix(c, paisley * (0.6 + 0.4 * rings), 0.35 + 0.6 * uAmt), 1.0);"#,
    },
    Filter {
        name: "Summertime",
        artist: "Janis Joplin",
        src: r#"
    float band = floor(vUv.y * mix(6.0, 30.0, uAmt));
    float off = sin(band * 1.7 + uTime * 2.0) * 0.04 * uAmt + sin(vUv.y * 120.0 + uTime * 8.0) * 0.003;
    vec3 c = vec3(cam(vUv + vec2(off * 1.4, 0.0)).r, cam(vUv + vec2(off, 0.0)).g, cam(vUv + vec2(off * 0.6, 0.0)).b);
    c = mix(c, c * vec3(1.1, 0.85, 0.95) + vec3(0.12, 0.05, 0.1), 0.6);
    gl_FragColor = vec4(c * (0.92 + 0.08 * sin(band * 3.0 + uTime * 4.0)), 1.0);"#,
    },
    Filter {
        name: "Move Over",
        artist: "Janis Joplin",
        src: r#"
    vec2 f = motion(vUv).gb;
    float speed = clamp(length(f) * 150.0 * (0.5 + uAmt), 0.0, 1.0);
    vec3 dir = hsv2rgb(vec3(atan(f.y, f.x) / 6.2831 + 0.5, 1.0, speed));
    gl_FragColor = vec4(max(cam(vUv) * 0.3 + dir, prev(vUv) * 0.85), 1.0);"#,
    },
    Filter {
        name: "Kozmic Blues",
        artist: "Janis Joplin",
        src: r#"
    vec4 m = motion(vUv);
    vec2 p = vUv - m.gb * (3.0 + 6.0 * uAmt) + 0.002 * vec2(sin(vUv.y * 12.0 + uTime), cos(vUv.x * 12.0 - uTime));
    vec3 ink = prev(p) * 0.985 + palette(uTime * 0.1 + vUv.x * 0.5) * smoothstep(0.05, 0.4, m.a) * 0.6;
    gl_FragColor = vec4(max(min(ink, vec3(1.2)), cam(vUv) * 0.2), 1.0);"#,
    },
    Filter {
        name: "Dark Side of the Moon",
        artist: "Pink Floyd",
        src: r#"
    vec2 dir = vec2(cos(uTime * 0.3), sin(uTime * 0.3)) * 0.04 * uAmt;
    vec3 acc = vec3(0.0), wsum = vec3(0.0);
    for (int i = 0; i < 12; i++) {
      float f = float(i) / 11.0;
      vec3 s = hsv2rgb(vec3(f * 0.8, 1.0, 1.0));
      acc += cam(vUv + dir * (f - 0.5)) * s;
      wsum += s;
    }
    vec3 c = acc / wsum;
    c = pow(c, vec3(1.3)) * smoothstep(0.08, 0.35, luma(c));
    gl_FragColor = vec4(c * 1.3, 1.0);"#,
    },
    Filter {
        name: "Echoes",
        artist: "Pink Floyd",
        src: r#"
    vec2 p = rot(0.02 + 0.04 * uAmt * sin(uTime * 0.4)) * (vUv - 0.5) * (0.97 - 0.04 * uAmt);
    vec3 h = rgb2hsv(prev(p + 0.5));
    h.x = fract(h.x + 0.03);
    h.y = min(1.0, h.y + 0.02);
    vec3 tunnel = hsv2rgb(h) * 0.985;
    vec3 c = cam(vUv);
    float edge = smoothstep(0.04, 0.2, length(grad(vUv)));
    gl_FragColor = vec4(mix(tunnel, c, max(edge, mix(0.3, 0.04, uAmt))), 1.0);"#,
    },
    Filter {
        name: "Interstellar Overdrive",
        artist: "Pink Floyd",
        src: r#"
    float shift = floor(1.0 + 7.0 * uAmt) / uRes.x;
    vec3 h = rgb2hsv(prev(vUv + vec2(shift, 0.0)));
    h.x = fract(h.x + 0.003);
    vec3 scan = vUv.x > 1.0 - shift ? cam(vec2(0.5, vUv.y)) : hsv2rgb(h);
    gl_FragColor = vec4(scan, 1.0);"#,
    },
    Filter {
        name: "Shine On You Crazy Diamond",
        artist: "Pink Floyd",
        src: r#"
    vec2 centre = uFocus + 0.08 * vec2(sin(uTime * 0.4), cos(uTime * 0.3));
    vec2 stride = (vUv - centre) * 0.03 * (0.5 + uAmt);
    vec3 acc = vec3(0.0);
    float weight = 1.0, total = 0.0;
    vec2 uv = vUv;
    for (int i = 0; i < 24; i++) {
      vec3 s = cam(uv);
      acc += s * smoothstep(0.45, 0.9, luma(s)) * weight;
      total += weight;
      weight *= 0.93;
      uv -= stride;
    }
    vec3 rays = acc / total * 3.0 * (0.6 + 0.4 * palette(length(vUv - centre) * 2.0 - uTime * 0.2));
    gl_FragColor = vec4(cam(vUv) * 0.55 + rays, 1.0);"#,
    },
    Filter {
        name: "Comfortably Numb",
        artist: "Pink Floyd",
        src: r#"
    float heat = motion(vUv).r;
    vec3 c = cam(vUv), h = rgb2hsv(c);
    h.x = fract(h.x + uTime * 0.2 + heat);
    h.y = 1.0;
    gl_FragColor = vec4(mix(vec3(luma(c)) * 0.6, hsv2rgb(h), smoothstep(0.0, 0.5, heat * (0.5 + uAmt))), 1.0);"#,
    },
    Filter {
        name: "Wish You Were Here",
        artist: "Pink Floyd",
        src: r#"
    vec2 f = motion(vUv).gb * (2.0 + 6.0 * uAmt);
    vec3 acc = vec3(0.0);
    for (int i = 0; i < 10; i++) acc += cam(vUv - f * float(i));
    vec3 smear = acc / 10.0;
    vec3 burn = mix(smear, smear * vec3(1.3, 0.8, 0.45) + vec3(0.15, 0.04, 0.0), smoothstep(0.05, 0.5, motion(vUv).r));
    gl_FragColor = vec4(mix(burn, prev(vUv - f * 2.0), 0.35), 1.0);"#,
    },
    Filter {
        name: "Time",
        artist: "Pink Floyd",
        src: r#"
    float moved = smoothstep(0.02, 0.5, motion(vUv).r);
    float delay = moved * 63.0 * (0.3 + 0.7 * uAmt);
    vec3 old = mix(past(vUv, floor(delay)), past(vUv, min(63.0, floor(delay) + 1.0)), fract(delay));
    vec3 c = mix(cam(vUv), old, smoothstep(0.0, 2.0, delay));
    gl_FragColor = vec4(mix(c, c * vec3(0.8, 0.95, 1.15), moved), 1.0);"#,
    },
    Filter {
        name: "Dragula",
        artist: "Rob Zombie",
        src: r#"
    float tick = floor(uTime * 12.0), line = floor(vUv.y * 90.0);
    float tear = step(0.92 - 0.25 * uAmt - 0.4 * uBurst, hash(vec2(line, tick))) * (hash(vec2(tick, line + 1.0)) - 0.5) * 0.25 * uAmt;
    vec2 uv = vUv + vec2(tear, 0.0);
    float split = 0.012 * uAmt * (1.0 + sin(uTime * 7.0));
    vec3 c = vec3(cam(uv + vec2(split, 0.0)).r, cam(uv).g, cam(uv - vec2(split, 0.0)).b);
    float l = luma(c);
    vec3 toxic = l > 0.6 ? vec3(0.85, 1.0, 0.3) : l > 0.32 ? vec3(0.85, 0.0, 0.06) : vec3(0.02, 0.0, 0.02);
    c = mix(c, toxic, 0.5 + 0.5 * uAmt);
    c *= 0.82 + 0.18 * sin(vUv.y * uRes.y * 1.6);
    c += (hash(vUv * uRes + fract(uTime)) - 0.5) * 0.18;
    c *= (0.88 + 0.12 * hash(vec2(tick, 3.0))) * (1.0 - 0.9 * length(vUv - 0.5));
    gl_FragColor = vec4(c, 1.0);"#,
    },
    Filter {
        name: "Living Dead Girl",
        artist: "Rob Zombie",
        src: r#"
    vec2 p = vUv - 0.5;
    float pulse = 0.5 + 0.5 * sin(uTime * 3.0);
    p *= 1.0 - 0.08 * uAmt * pulse * smoothstep(0.6, 0.0, length(p));
    vec3 c = cam(p + 0.5);
    float l = luma(c);
    vec3 duo = mix(vec3(0.0, 0.08, 0.05), vec3(0.6, 1.0, 0.75), smoothstep(0.2, 0.75, l));
    duo = mix(duo, vec3(1.0, 0.0, 0.15), step(0.97 - 0.15 * uAmt, hash(floor(vUv * 40.0) + floor(uTime * 6.0))) * step(l, 0.4));
    vec3 ghost = prev(vUv + vec2(0.002 * sin(uTime * 2.0), 0.0)) * 0.85;
    gl_FragColor = vec4(max(duo, ghost * uAmt), 1.0);"#,
    },
    Filter {
        name: "Superbeast",
        artist: "Rob Zombie",
        src: r#"
    float size = mix(12.0, 48.0, uAmt);
    vec2 blk = floor(vUv * uRes / size);
    float tick = floor(uTime * 8.0);
    vec2 drift = (vec2(hash(blk + tick + 1.3), hash(blk + tick + 7.1)) - 0.5) * 0.02;
    vec3 o = mix(cam(vUv), prev(vUv + drift), step(1.0 - 0.6 * uAmt - 0.3 * uBurst, hash(blk + tick)));
    if (hash(blk * 1.7 + tick) > 0.93) o = cam((blk + 0.5) * size / uRes).brg;
    gl_FragColor = vec4(o, 1.0);"#,
    },
    Filter {
        name: "Meet the Creeper",
        artist: "Rob Zombie",
        src: r#"
    vec2 block = 16.0 / uRes;
    vec4 m = motion((floor(vUv / block) + 0.5) * block);
    vec3 moshed = prev(vUv - m.gb * (1.5 + 2.5 * uAmt));
    float refresh = max(step(0.85, uBurst), 0.015 + 0.1 * (1.0 - uAmt) * (1.0 - m.a));
    gl_FragColor = vec4(mix(moshed, cam(vUv), refresh), 1.0);"#,
    },
    Filter {
        name: "Thunder Kiss '65 (flashing)",
        artist: "Rob Zombie",
        src: r#"
    float beats = uClock * 2.5;
    float beat = fract(beats), tick = floor(beats);
    vec3 c = cam(vUv);
    vec3 flash = hsv2rgb(vec3(hash(vec2(tick, 1.0)), 0.9, 1.0));
    vec3 o = mix(c, step(0.45, luma(c)) * flash, 0.4 + 0.6 * uAmt);
    gl_FragColor = vec4(o * (1.0 + 0.6 * uAmt * exp(-beat * 10.0)), 1.0);"#,
    },
];
