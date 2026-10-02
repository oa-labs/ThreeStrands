import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, it } from "node:test";
import sharp from "sharp";
import { optimizeScreenshots } from "./optimize-screenshots.mjs";

describe("screenshot conversion", () => {
  async function fixture(run) {
    const dir = await mkdtemp(path.join(tmpdir(), "threestrands-shots-"));
    const sourceDir = path.join(dir, "captures");
    const outputDir = path.join(dir, "output");
    await mkdir(outputDir);
    await writeFile(path.join(outputDir, "previous.txt"), "previous assets");
    for (const theme of ["light", "dark"]) await mkdir(path.join(sourceDir, theme), { recursive: true });
    const capture = (theme, width = 2880, height = 1800) => sharp({ create: { width, height, channels: 3, background: "#333183" } })
      .png().toFile(path.join(sourceDir, theme, "inbox.png"));
    try { await run({ sourceDir, outputDir, requiredScenes: ["inbox"] }, capture); }
    finally { await rm(dir, { recursive: true, force: true }); }
  }

  it("rejects incomplete themes without removing existing assets", async () => {
    await fixture(async (options, capture) => {
      await capture("light");
      await assert.rejects(optimizeScreenshots(options), /same nonempty scene set/);
      assert.equal(await readFile(path.join(options.outputDir, "previous.txt"), "utf8"), "previous assets");
    });
  });

  it("rejects a missing site scene even when themes match", async () => {
    await fixture(async (options, capture) => {
      await Promise.all([capture("light"), capture("dark")]);
      await assert.rejects(optimizeScreenshots({ ...options, requiredScenes: ["inbox", "thread-chat"] }), /Missing required screenshot: thread-chat/);
      assert.equal(await readFile(path.join(options.outputDir, "previous.txt"), "utf8"), "previous assets");
    });
  });

  it("rejects undersized and incorrectly shaped captures without replacing assets", async () => {
    for (const [width, height] of [[1440, 900], [2880, 1799]]) {
      await fixture(async (options, capture) => {
        await Promise.all([capture("light", width, height), capture("dark", width, height)]);
        await assert.rejects(optimizeScreenshots(options), /must be 2880×1800/);
        assert.equal(await readFile(path.join(options.outputDir, "previous.txt"), "utf8"), "previous assets");
      });
    }
  });

  it("converts an overridden source directory to all theme, width, and format variants", async () => {
    await fixture(async (options, capture) => {
      await Promise.all([capture("light"), capture("dark")]);
      const previous = process.env.SCREENSHOT_DIR;
      process.env.SCREENSHOT_DIR = options.sourceDir;
      try {
        assert.equal(await optimizeScreenshots({ outputDir: options.outputDir, requiredScenes: ["inbox"] }), 12);
      } finally {
        if (previous === undefined) delete process.env.SCREENSHOT_DIR;
        else process.env.SCREENSHOT_DIR = previous;
      }
      for (const theme of ["light", "dark"]) {
        for (const width of [800, 1440, 2880]) {
          for (const format of ["avif", "webp"]) {
            const metadata = await sharp(path.join(options.outputDir, theme, `inbox-${width}.${format}`)).metadata();
            assert.equal(metadata.width, width);
            assert.equal(metadata.height, width * 900 / 1440);
          }
        }
      }
    });
  });
});
