import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const root = resolve(import.meta.dirname, "..");
const read = (path: string) => readFileSync(resolve(root, path), "utf8");

describe("application icon assets", () => {
  it("ships the unmasked macOS Icon Composer document with an ICNS fallback", () => {
    const config = JSON.parse(read("src-tauri/tauri.conf.json"));

    expect(config.bundle.icon).toContain("../assets/AppIcon.icon");
    expect(config.bundle.icon).toContain("icons/icon.icns");
  });

  it("uses separate, square, unmasked postmark layers", () => {
    const document = JSON.parse(read("assets/AppIcon.icon/icon.json"));
    const layerNames = document.groups.flatMap(
      (group: { layers: Array<{ "image-name": string }> }) =>
        group.layers.map((layer) => layer["image-name"]),
    );

    expect(document["supported-platforms"].squares).toContain("macOS");
    expect(layerNames).toEqual(["postmark-rings.svg", "strands.svg"]);

    for (const name of layerNames) {
      const svg = read(`assets/AppIcon.icon/Assets/${name}`);
      expect(svg).toContain('viewBox="0 0 1024 1024"');
      expect(svg).not.toMatch(/<(?:rect|clipPath|mask)\b/);
    }
  });

  it("provides authored dark and mono/tinted variants", () => {
    const document = JSON.parse(read("assets/AppIcon.icon/icon.json"));
    const appearances = document["fill-specializations"].map(
      (entry: { appearance: string }) => entry.appearance,
    );

    expect(appearances).toEqual(["dark", "tinted"]);

    const specializedAssets = document.groups.flatMap(
      (group: {
        layers: Array<{
          "image-name-specializations": Array<{
            appearance: string;
            value: string;
          }>;
        }>;
      }) =>
        group.layers.flatMap((layer) => layer["image-name-specializations"]),
    );

    expect(specializedAssets).toHaveLength(4);
    expect(
      specializedAssets.map((entry: { appearance: string }) => entry.appearance),
    ).toEqual([
      "dark",
      "tinted",
      "dark",
      "tinted",
    ]);

    for (const { value } of specializedAssets) {
      const svg = read(`assets/AppIcon.icon/Assets/${value}`);
      expect(svg).toContain('viewBox="0 0 1024 1024"');
      expect(svg).not.toMatch(/<(?:rect|clipPath|mask)\b/);
    }

    const monoStrands = read("assets/AppIcon.icon/Assets/strands-mono.svg");
    expect(monoStrands).toContain('stroke="#ffffff"');
    expect(monoStrands).not.toMatch(/stroke="#(?:9f8bcb|f3eae9|fdad81)"/i);
  });

  it("keeps the postmark strokes legible at small icon sizes", () => {
    const rings = read("assets/AppIcon.icon/Assets/postmark-rings.svg");
    const strands = read("assets/AppIcon.icon/Assets/strands.svg");

    expect(rings).toContain('stroke-width="48"');
    expect(rings).toContain('stroke-width="28"');
    expect(strands).toContain('stroke-width="42"');
  });
});
