// Converts the showcase captures from `pnpm screenshots` into the responsive
// AVIF/WebP set the marketing site serves. The output is committed so a
// site deploy never needs Chromium; rerun after regenerating screenshots:
//
//   pnpm screenshots && pnpm site:images
import { mkdir, readdir, rm, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import sharp from "sharp";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const sourceDir = path.join(root, "artifacts", "screenshots");
const outputDir = path.join(root, "site", "src", "assets", "screenshots");
const widths = [800, 1440, 2880];
const formats = {
  avif: (image) => image.avif({ quality: 58, effort: 6 }),
  webp: (image) => image.webp({ quality: 80, effort: 6 }),
};

async function exists(target) {
  try {
    await stat(target);
    return true;
  } catch {
    return false;
  }
}

if (!(await exists(sourceDir))) {
  console.error(`No screenshots in ${path.relative(root, sourceDir)}. Run \`pnpm screenshots\` first.`);
  process.exit(1);
}

await rm(outputDir, { recursive: true, force: true });
let written = 0;
for (const theme of ["light", "dark"]) {
  const themeDir = path.join(sourceDir, theme);
  const scenes = (await readdir(themeDir)).filter((name) => name.endsWith(".png")).sort();
  await mkdir(path.join(outputDir, theme), { recursive: true });
  for (const file of scenes) {
    const scene = path.basename(file, ".png");
    const source = sharp(path.join(themeDir, file));
    const { width } = await source.metadata();
    for (const target of widths) {
      if (target > width) continue;
      for (const [extension, encode] of Object.entries(formats)) {
        const output = path.join(outputDir, theme, `${scene}-${target}.${extension}`);
        await encode(source.clone().resize({ width: target })).toFile(output);
        written += 1;
      }
    }
  }
}
console.log(`Wrote ${written} images to ${path.relative(root, outputDir)}`);
