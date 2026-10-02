// Converts fictional showcase captures into the committed responsive image set.
// Run pnpm screenshots && pnpm site:images; both honor SCREENSHOT_DIR.
import { mkdir, mkdtemp, readFile, readdir, rename, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import sharp from "sharp";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const widths = [800, 1440, 2880];
const formats = {
  avif: (image) => image.avif({ quality: 58, effort: 6 }),
  webp: (image) => image.webp({ quality: 80, effort: 6 }),
};

export async function optimizeScreenshots({
  sourceDir = path.resolve(root, process.env.SCREENSHOT_DIR ?? "artifacts/screenshots"),
  outputDir = path.join(root, "site/src/assets/screenshots"),
  requiredScenes,
} = {}) {
  if (!requiredScenes) {
    const html = await readFile(path.join(root, "site/index.html"), "utf8");
    requiredScenes = [...new Set([...html.matchAll(/<ts-shot\b[^>]*\bscene="([\w-]+)"/g)].map((match) => match[1]))];
  }
  const themes = ["light", "dark"];
  const files = await Promise.all(themes.map(async (theme) => {
    try {
      return (await readdir(path.join(sourceDir, theme))).filter((name) => name.endsWith(".png")).sort();
    } catch (error) {
      throw new Error(`Cannot read ${theme} captures in ${sourceDir}. Run pnpm screenshots first.`, { cause: error });
    }
  }));
  if (files[0].length === 0 || JSON.stringify(files[0]) !== JSON.stringify(files[1])) {
    throw new Error("Screenshot captures must have the same nonempty scene set in light and dark.");
  }
  for (const scene of requiredScenes) {
    if (!files[0].includes(`${scene}.png`)) throw new Error(`Missing required screenshot: ${scene}. Run pnpm screenshots first.`);
  }
  // Validate every input before touching committed output. Captures are 2x 1440×900.
  for (const theme of themes) {
    for (const file of files[0]) {
      const { width, height } = await sharp(path.join(sourceDir, theme, file)).metadata();
      if (width !== 2880 || height !== 1800) {
        throw new Error(`${theme}/${file} must be 2880×1800; got ${width}×${height}.`);
      }
    }
  }

  const parent = path.dirname(outputDir);
  await mkdir(parent, { recursive: true });
  const staging = await mkdtemp(path.join(parent, ".screenshots-"));
  let written = 0;
  try {
    for (const theme of themes) {
      await mkdir(path.join(staging, theme));
      for (const file of files[0]) {
        const scene = path.basename(file, ".png");
        const source = sharp(path.join(sourceDir, theme, file));
        for (const width of widths) {
          for (const [extension, encode] of Object.entries(formats)) {
            await encode(source.clone().resize({ width })).toFile(path.join(staging, theme, `${scene}-${width}.${extension}`));
            written += 1;
          }
        }
      }
    }
    // Conversion failures leave the previous assets untouched.
    await rm(outputDir, { recursive: true, force: true });
    await rename(staging, outputDir);
    return written;
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    const written = await optimizeScreenshots();
    console.log(`Wrote ${written} images to site/src/assets/screenshots`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
