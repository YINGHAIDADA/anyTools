// Generates the menu-bar template icon (black + alpha, macOS tints it): a ring of 6 sectors with one
// highlighted. Output: 36x36 PNG (18pt @2x). Usage: node make-tray-icon.mjs <out.png>
import { deflateSync } from 'node:zlib';
import { writeFileSync } from 'node:fs';

const S = 36;
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

const SS = 4; // supersampling
const px = Buffer.alloc(S * (S * 4 + 1));
const n = 6;
for (let y = 0; y < S; y++) {
  px[y * (S * 4 + 1)] = 0;
  for (let x = 0; x < S; x++) {
    let cover = 0;
    for (let sy = 0; sy < SS; sy++) {
      for (let sx = 0; sx < SS; sx++) {
        const dx = x + (sx + 0.5) / SS - S / 2;
        const dy = y + (sy + 0.5) / SS - S / 2;
        const r = Math.hypot(dx, dy);
        let a = Math.atan2(dx, -dy);
        if (a < 0) a += Math.PI * 2;
        const w = (Math.PI * 2) / n;
        const local = ((a + w / 2) % w) - w / 2; // angle from the sector's center line
        const inSector = Math.floor((a + w / 2) / w) % n;
        // The top sector is "selected": it reaches further in.
        const inner = inSector === 0 ? 6 : 8.5;
        // Distance to the nearest sector boundary line → a ~2px gap between sectors.
        const toEdge = r * Math.sin(w / 2 - Math.abs(local));
        const ring = r >= inner && r <= 16.5 && toEdge > 1.1;
        const dot = r <= 2.8;
        if (ring || dot) cover++;
      }
    }
    const o = y * (S * 4 + 1) + 1 + x * 4;
    px[o] = px[o + 1] = px[o + 2] = 0;
    px[o + 3] = Math.round((255 * cover) / (SS * SS));
  }
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8;
ihdr[9] = 6;
writeFileSync(
  process.argv[2] ?? 'tray.png',
  Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', ihdr), chunk('IDAT', deflateSync(px)), chunk('IEND', Buffer.alloc(0))]),
);
