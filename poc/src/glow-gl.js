// WebGL renderer for the flowing edge glow: one quad, an SDF of the hovered annular sector,
// with a hue band that travels around the outline. Used as the alternative to the CSS renderer (P4).

const VS = `
attribute vec2 p;
void main() { gl_Position = vec4(p, 0.0, 1.0); }
`;

const FS = `
precision highp float;
uniform vec2 res;
uniform float t;
uniform float mid;      // sector center angle (rad, clockwise from 12 o'clock)
uniform float halfw;    // half angular width (rad)
uniform float rin;
uniform float rout;
uniform float gap;
uniform float alpha;
uniform float cursorA;  // cursor angle, the glow brightens towards it

const float PI = 3.14159265;

float wrap(float a) { return mod(a + PI, 2.0 * PI) - PI; }

void main() {
  // window space, y-down, origin at wheel center, in CSS px
  vec2 q = (gl_FragCoord.xy / res) * 420.0;
  vec2 d = vec2(q.x - 210.0, 210.0 - q.y);
  float r = length(d);
  float a = atan(d.x, d.y);                 // clockwise from 12 o'clock
  float da = wrap(a - mid);

  // signed distance (approx.) to the annular sector with straight gap edges; negative inside
  float dr = max(rin - r, r - rout);
  float edge = abs(da) - halfw;
  float dEdge = r * sin(clamp(edge, -PI * 0.5, PI * 0.5)) + gap * 0.5;
  float sd = max(dr, dEdge);

  float line = exp(-pow(sd / 1.1, 2.0));
  float bloom = exp(-abs(sd) / 7.0) * 0.55;

  // travelling band along the outline + bias towards the cursor direction
  float s = a / (2.0 * PI) - t * 0.42;
  float band = 0.55 + 0.45 * sin(s * 2.0 * PI * 2.0);
  float toward = 0.65 + 0.35 * cos(wrap(a - cursorA));

  vec3 c1 = vec3(0.545, 0.424, 1.0);
  vec3 c2 = vec3(0.133, 0.827, 0.933);
  vec3 c3 = vec3(0.957, 0.447, 0.714);
  float h = fract(s);
  vec3 col = h < 0.5 ? mix(c1, c2, h * 2.0) : mix(c2, c3, (h - 0.5) * 2.0);
  col = mix(col, c1, 0.25);

  float k = (line + bloom) * band * toward * alpha;
  gl_FragColor = vec4(col * k, k);          // premultiplied
}
`;

export function createGlGlow(canvas) {
  const gl = canvas.getContext('webgl', { premultipliedAlpha: true, alpha: true, antialias: false });
  if (!gl) return null;
  const sh = (type, src) => {
    const s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s));
    return s;
  };
  const prog = gl.createProgram();
  gl.attachShader(prog, sh(gl.VERTEX_SHADER, VS));
  gl.attachShader(prog, sh(gl.FRAGMENT_SHADER, FS));
  gl.linkProgram(prog);
  gl.useProgram(prog);
  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(prog, 'p');
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  gl.enable(gl.BLEND);
  gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
  const u = Object.fromEntries(
    ['res', 't', 'mid', 'halfw', 'rin', 'rout', 'gap', 'alpha', 'cursorA'].map((n) => [n, gl.getUniformLocation(prog, n)]),
  );

  return {
    draw({ t, mid, halfw, geom, alpha, cursorA }) {
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      if (alpha <= 0.001) return;
      gl.uniform2f(u.res, canvas.width, canvas.height);
      gl.uniform1f(u.t, t);
      gl.uniform1f(u.mid, mid);
      gl.uniform1f(u.halfw, halfw);
      gl.uniform1f(u.rin, geom.r_in);
      gl.uniform1f(u.rout, geom.r_out);
      gl.uniform1f(u.gap, geom.gap);
      gl.uniform1f(u.alpha, alpha);
      gl.uniform1f(u.cursorA, cursorA);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    },
    clear() {
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
    },
  };
}
