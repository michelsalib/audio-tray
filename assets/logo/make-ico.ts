/**
 * Renders the logo to assets/app.ico (embedded in the exe by build.rs) and, on request, to PNGs.
 *
 *   bun assets/logo/make-ico.ts                       default blue, writes assets/app.ico
 *   bun assets/logo/make-ico.ts --accent bd5bcb,9e3ab0,460d5a --png out-dir
 *                                                     another accent (Light1,Dark1,Dark3),
 *                                                     PNGs only, app.ico left alone
 *
 * Needs `sharp` resolvable (it renders the SVG with librsvg): run it from a directory where
 * `bun add sharp` was done, or with NODE_PATH pointing at one. Nothing in the Rust build uses it.
 *
 * The colour roles are computed here rather than through audio-tray-accent.svg's CSS, because
 * librsvg has no color-mix(). Same arithmetic, so this is also the reference for the app's own
 * rendering: plain sRGB mixes of AccentPalette shades.
 */

import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import sharp from "sharp";

const HERE = import.meta.dir;
const SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256];

type Rgb = [number, number, number];
const hex = (s: string): Rgb => {
  const n = Number.parseInt(s.replace("#", ""), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
};
const css = (c: Rgb) => `#${c.map((v) => Math.round(v).toString(16).padStart(2, "0")).join("")}`;
/** `amount` of `a` over `b`, in sRGB, as CSS color-mix(in srgb, a amount, b) does. */
const mix = (a: Rgb, amount: number, b: Rgb): Rgb =>
  a.map((v, i) => v * amount + (b[i] as number) * (1 - amount)) as Rgb;

/** The logo's four colours from three AccentPalette shades. */
export function roles(light1: string, dark1: string, dark3: string) {
  return {
    tileFrom: css(mix(hex(light1), 0.12, [255, 255, 255])),
    tileTo: css(mix(hex(light1), 0.22, [255, 255, 255])),
    glyph: css(mix(hex(dark3), 0.55, [0x1c, 0x1c, 0x1c])),
    bar: css(hex(dark1)),
  };
}

const args = process.argv.slice(2);
const flag = (name: string) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};
const [light1 = "0091f8", dark1 = "0067c0", dark3 = "001a68"] =
  flag("--accent")?.split(",") ?? [];
const role = roles(light1, dark1, dark3);
const pngDir = flag("--png");

// audio-tray.svg carries the default roles as literal colours; swap them for this accent's.
const DEFAULT = roles("0091f8", "0067c0", "001a68");
let svg = await Bun.file(join(HERE, "audio-tray.svg")).text();
for (const key of Object.keys(DEFAULT) as (keyof typeof DEFAULT)[]) {
  svg = svg.replaceAll(DEFAULT[key], role[key]);
}

const pngs = await Promise.all(
  SIZES.map((size) =>
    // Small sizes straight at their size (librsvg's own antialiasing is crisper than a
    // downscale); large ones supersampled.
    sharp(Buffer.from(svg), { density: ((72 * size) / 1024) * (size <= 48 ? 1 : 4) })
      .resize(size, size)
      .png()
      .toBuffer(),
  ),
);

if (pngDir) {
  await mkdir(pngDir, { recursive: true });
  await Promise.all(pngs.map((png, i) => Bun.write(join(pngDir, `audio-tray-${SIZES[i]}.png`), png)));
  console.log(`${pngs.length} PNGs in ${pngDir}`, role);
} else {
  // ICO: a 6-byte header, a 16-byte entry per image, then the images, PNG-compressed (Vista+,
  // which the previous app.ico used too).
  const header = Buffer.alloc(6 + 16 * SIZES.length);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(SIZES.length, 4);
  let offset = header.length;
  SIZES.forEach((size, i) => {
    const e = 6 + 16 * i;
    const png = pngs[i] as Buffer;
    header.writeUInt8(size >= 256 ? 0 : size, e);
    header.writeUInt8(size >= 256 ? 0 : size, e + 1);
    header.writeUInt16LE(1, e + 4); // planes
    header.writeUInt16LE(32, e + 6); // bits per pixel
    header.writeUInt32LE(png.length, e + 8);
    header.writeUInt32LE(offset, e + 12);
    offset += png.length;
  });
  const out = join(HERE, "..", "app.ico");
  await Bun.write(out, Buffer.concat([header, ...pngs]));
  console.log(`${out}: ${SIZES.join(", ")} px`);
}
