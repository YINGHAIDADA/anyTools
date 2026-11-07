// Generates a placeholder 1024x1024 app icon (glowing ring) as PNG, used as the source for `tauri icon`.
import { deflateSync } from 'node:zlib';
import { writeFileSync } from 'node:fs';

const S = 1024;
const px = Buffer.alloc(S * (S * 4 + 1));
const crcTable = new Uint32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc = (buf) => {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const c = Buffer.alloc(4);
  c.writeUInt32BE(crc(td));
  return Buffer.concat([len, td, c]);
};

const smooth = (e0, e1, x) => {
  const t = Math.min(1, Math.max(0, (x - e0) / (e1 - e0)));
  return t * t * (3 - 2 * t);
};

for (let y = 0; y < S; y++) {
  px[y * (S * 4 + 1)] = 0;
  for (let x = 0; x < S; x++) {
    const dx = x - S / 2 + 0.5, dy = y - S / 2 + 0.5;
    const d = Math.hypot(dx, dy);
    const a = Math.atan2(dx, -dy);
    // rounded-square background (SDF)
    const half = 450, rc = 180, inner = half - rc;
    const qx = Math.abs(dx) - inner, qy = Math.abs(dy) - inner;
    const sd = Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - rc;
    const bg = 1 - smooth(-1, 1, sd);
    const ring = Math.exp(-(((d - 300) / 34) ** 2));
    const glow = Math.exp(-(((d - 300) / 110) ** 2)) * 0.45;
    const t = (a / (2 * Math.PI) + 1) % 1;
    const r = 124 + 100 * Math.cos(2 * Math.PI * t);
    const g = 92 + 120 * Math.cos(2 * Math.PI * (t - 0.33));
    const b = 255 - 40 * Math.cos(2 * Math.PI * (t - 0.66));
    const k = Math.min(1, ring + glow);
    const o = y * (S * 4 + 1) + 1 + x * 4;
    px[o] = Math.round(18 + (r - 18) * k);
    px[o + 1] = Math.round(18 + (g - 18) * k);
    px[o + 2] = Math.round(28 + (b - 28) * k);
    px[o + 3] = Math.round(255 * bg);
  }
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8;
ihdr[9] = 6;
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk('IHDR', ihdr),
  chunk('IDAT', deflateSync(px)),
  chunk('IEND', Buffer.alloc(0)),
]);
writeFileSync(process.argv[2] ?? 'icon-src.png', png);
