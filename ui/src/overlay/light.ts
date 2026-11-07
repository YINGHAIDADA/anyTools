// WebGL2 hover light, in the style of the Apple Intelligence edge glow: a soft, saturated gradient that
// travels around the outline of the hovered sector, bleeding a little light inward and outward.
// Nothing else — the glass is native and is left untouched.
//
// Geometry mirrors crates/modules/wheel/src/geometry.rs.

import type { Geom } from '../shared/types';

const VS = `#version 300 es
in vec2 p;
void main() { gl_Position = vec4(p, 0.0, 1.0); }
`;

const FS = `#version 300 es
precision highp float;
uniform vec2 uRes;
uniform float uDpr;
uniform float uWin;
uniform float uTime;
uniform float uN;
uniform vec4 uGeom;     // r_out, r_in, r_center, gap
uniform vec3 uH0;       // hovered sector: index, amount, age (s)
uniform vec2 uH1;       // previous sector fading out: index, amount
uniform float uFlash;
uniform float uLight;   // light theme
uniform vec3 uPal[5];
out vec4 o;

const float PI = 3.14159265, TAU = 6.28318531;

float hash(vec2 p) { p = fract(p * vec2(123.34, 456.21)); p += dot(p, p + 45.32); return fract(p.x * p.y); }
float noise(vec2 p) {
  vec2 i = floor(p), f = fract(p), u = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1, 0)), u.x), mix(hash(i + vec2(0, 1)), hash(i + vec2(1, 1)), u.x), u.y);
}
float wrapA(float a) { return mod(a + PI, TAU) - PI; }

// Smooth palette loop through the five glow colors.
vec3 pal(float h) {
  h = fract(h) * 5.0;
  int i = int(floor(h));
  float f = smoothstep(0.0, 1.0, fract(h));
  return mix(uPal[i], uPal[(i + 1) % 5], f);
}

// Signed distance (px, positive inside) to sector idx, with softly rounded corners.
float sectorSd(vec2 d, float idx, out float param) {
  float w = TAU / uN;
  float r = length(d);
  float a = atan(d.x, -d.y);
  float la = wrapA(a - idx * w);
  float side = r * sin(clamp(w * 0.5 - abs(la), -PI * 0.5, PI * 0.5)) - uGeom.w * 0.5;
  vec3 e = vec3(side, uGeom.x - r, r - uGeom.y);
  e = max(e, vec3(-80.0));
  const float k = 5.0;
  float s = -k * log(exp(-e.x / k) + exp(-e.y / k) + exp(-e.z / k));
  // Position along the outline (0..1), measured around the sector's centroid: drives the flow.
  float rm = (uGeom.x + uGeom.y) * 0.5;
  vec2 cen = vec2(sin(idx * w), -cos(idx * w)) * rm;
  vec2 q = d - cen;
  param = atan(q.x, -q.y) / TAU + 0.5;
  return s;
}

// The glow around one sector, as premultiplied color.
vec4 glow(vec2 d, float idx, float amt, float age) {
  if (amt <= 0.001) return vec4(0.0);
  float param;
  float s = sectorSd(d, idx, param);
  float t = uTime;
  // Colors travel around the outline; brightness drifts in soft patches that move with them.
  float flow = param - t * 0.16;
  vec3 c = pal(flow + idx * 0.13);
  // Two colors at once across the band (edge vs. inner bleed), like the Siri glow's layered tones.
  vec3 c2 = pal(flow + 0.33 + idx * 0.13);
  float drift = 0.6 + 0.4 * noise(vec2(param * 5.0 - t * 0.8, t * 0.22 + idx));
  // Width breathes, and grows in as the hover starts.
  float grow = smoothstep(0.0, 0.4, age);
  float wIn = (15.0 + 7.0 * noise(vec2(param * 3.0 + t * 0.35, 7.0 + idx))) * (0.5 + 0.5 * grow);
  float line = exp(-abs(s) / 1.6);                                  // the luminous edge itself
  float inner = s > 0.0 ? exp(-s / wIn) : 0.0;                      // light bleeding into the glass
  float outer = s < 0.0 ? exp(s / 11.0) : 0.0;                      // halo outside
  float iLine = line * 0.95, iIn = inner * 0.7, iOut = outer * 0.55;
  float I = (iLine + iIn + iOut) * drift * amt;
  I *= mix(1.0, 0.92, uLight);
  // Edge takes one hue, the bleed shifts toward the next: saturated, never washed out.
  c = mix(c2, c, clamp(iLine / max(iLine + iIn + iOut, 1e-3) + 0.25, 0.0, 1.0));
  // Saturation boost (glow over glass reads paler than the raw palette).
  float lum = dot(c, vec3(0.299, 0.587, 0.114));
  c = clamp(mix(vec3(lum), c, 1.35), 0.0, 1.0);
  // Only the thinnest core of the edge line lifts toward white.
  c = mix(c, vec3(1.0), pow(line, 3.0) * 0.18);
  // Running an item: a brief bright wash over the sector.
  float f = uFlash * smoothstep(-1.0, 1.0, s) * 0.45;
  vec3 col = c * I + vec3(1.0) * f;
  float a = clamp(I * 0.95 + f, 0.0, 1.0);
  return vec4(col, a);
}

void main() {
  vec2 px = vec2(gl_FragCoord.x, uRes.y - gl_FragCoord.y) / uDpr;
  vec2 d = px - vec2(uWin * 0.5);
  vec4 g0 = glow(d, uH0.x, uH0.y, uH0.z);
  vec4 g1 = abs(uH1.x - uH0.x) > 0.5 ? glow(d, uH1.x, uH1.y, 10.0) : vec4(0.0);
  vec4 c = g0 + g1 * (1.0 - g0.a);
  o = vec4(min(c.rgb, vec3(1.0)), clamp(c.a, 0.0, 1.0));
}
`;

export type Rgb = [number, number, number];

export interface LightState {
  n: number;
  h0: number;
  h0Amount: number;
  h0Age: number;
  h1: number;
  h1Amount: number;
  flash: number;
  light: boolean;
  palette: Rgb[];
  time: number;
}

export interface LightRenderer {
  draw(s: LightState): void;
}

export function createLight(canvas: HTMLCanvasElement, geom: Geom): LightRenderer | null {
  const gl = canvas.getContext('webgl2', { premultipliedAlpha: true, alpha: true, antialias: false });
  if (!gl) return null;
  const compile = (type: number, src: string) => {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? 'shader error');
    return s;
  };
  const prog = gl.createProgram()!;
  gl.attachShader(prog, compile(gl.VERTEX_SHADER, VS));
  gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, FS));
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog) ?? 'link error');
  gl.useProgram(prog);
  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(prog, 'p');
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  const u = (n: string) => gl.getUniformLocation(prog, n);
  const U = {
    res: u('uRes'), dpr: u('uDpr'), win: u('uWin'), time: u('uTime'), n: u('uN'), geom: u('uGeom'), h0: u('uH0'), h1: u('uH1'),
    flash: u('uFlash'), light: u('uLight'), pal: u('uPal'),
  };
  let dpr = 0;
  let drewEmpty = false;
  const resize = () => {
    const want = Math.min(window.devicePixelRatio || 1, 3);
    if (want === dpr) return;
    dpr = want;
    canvas.width = Math.round(geom.win * dpr);
    canvas.height = Math.round(geom.win * dpr);
  };
  resize();
  return {
    draw(s) {
      resize();
      const idle = s.h0Amount <= 0.001 && s.h1Amount <= 0.001 && s.flash <= 0.001;
      // Nothing hovered: one clear frame, then skip the shader entirely.
      if (idle && drewEmpty) return;
      drewEmpty = idle;
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      if (idle) return;
      gl.uniform2f(U.res, canvas.width, canvas.height);
      gl.uniform1f(U.dpr, dpr);
      gl.uniform1f(U.win, geom.win);
      gl.uniform1f(U.time, s.time);
      gl.uniform1f(U.n, s.n);
      gl.uniform4f(U.geom, geom.r_out, geom.r_in, geom.r_center, geom.gap);
      gl.uniform3f(U.h0, s.h0, s.h0Amount, s.h0Age);
      gl.uniform2f(U.h1, s.h1, s.h1Amount);
      gl.uniform1f(U.flash, s.flash);
      gl.uniform1f(U.light, s.light ? 1 : 0);
      gl.uniform3fv(U.pal, s.palette.flat());
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    },
  };
}

// ------------------------------------------------------------------------------------------------
// Palette. Default: the Apple Intelligence glow colors. A custom accent tints them toward it.

const AI: Rgb[] = ['#8d9fff', '#bc82f3', '#f5b9ea', '#ff6778', '#ffba71'].map(hex);

function hex(h: string): Rgb {
  const s = h.replace('#', '');
  const f = s.length === 3 ? s.split('').map((c) => c + c).join('') : s.slice(0, 6);
  return [0, 2, 4].map((i) => parseInt(f.slice(i, i + 2), 16) / 255) as Rgb;
}

function mix(a: Rgb, b: Rgb, t: number): Rgb {
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
}

/** Any CSS color → rgb, via a 2D canvas (handles names, rgb(), hsl()). */
function cssColor(c: string): Rgb | null {
  const ctx = document.createElement('canvas').getContext('2d');
  if (!ctx) return null;
  ctx.fillStyle = '#000';
  ctx.fillStyle = c;
  const v = String(ctx.fillStyle);
  if (v.startsWith('#')) return hex(v);
  const m = v.match(/[\d.]+/g);
  return m && m.length >= 3 ? [Number(m[0]) / 255, Number(m[1]) / 255, Number(m[2]) / 255] : null;
}

export function palette(accent: string | undefined): Rgb[] {
  if (!accent || accent === 'system') return AI;
  const a = cssColor(accent);
  if (!a) return AI;
  // Keep the flow's variety, centred on the accent.
  return [mix(a, AI[0], 0.25), a, mix(a, AI[2], 0.35), mix(a, AI[3], 0.25), mix(a, [1, 1, 1], 0.35)];
}
