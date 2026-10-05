import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";

const siteRoot = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(siteRoot, "..");
const { version } = JSON.parse(readFileSync(path.join(repoRoot, "package.json"), "utf8")) as { version: string };
// The same Pages artifact must work at both a custom-domain root and the
// repository path. Root-absolute assets fail at the latter when Pages reports
// the custom domain's empty base_path.
const siteBase = process.env.SITE_BASE ?? "./";

/** Must match the widths `scripts/optimize-screenshots.mjs` writes. */
const SHOT_WIDTHS = [800, 1440, 2880];
const SHOT_RATIO = { width: 1440, height: 900 };

function attributes(tag: string): Record<string, string> {
  const result: Record<string, string> = {};
  for (const match of tag.matchAll(/([\w-]+)(?:="([^"]*)")?/g)) result[match[1]!] = match[2] ?? "";
  return result;
}

function srcset(theme: string, scene: string, format: string) {
  return SHOT_WIDTHS.map((width) => `/src/assets/screenshots/${theme}/${scene}-${width}.${format} ${width}w`).join(", ");
}

/**
 * Expands `<ts-shot scene="inbox" alt="…">` into a responsive `<picture>`.
 * `theme="auto"` (the default) follows the visitor's color scheme; `dark` or
 * `light` pins one variant. Everything lazy-loads unless `eager` is set.
 */
function shot(attrs: Record<string, string>) {
  const { scene, alt, theme = "auto", sizes = "(min-width: 1200px) 1100px, 92vw" } = attrs;
  if (!scene || alt === undefined) throw new Error(`<ts-shot> needs scene and alt: ${JSON.stringify(attrs)}`);
  const eager = "eager" in attrs;
  const fallbackTheme = theme === "auto" ? "light" : theme;
  const sources: string[] = [];
  if (theme === "auto") {
    for (const format of ["avif", "webp"]) {
      sources.push(`<source type="image/${format}" media="(prefers-color-scheme: dark)" srcset="${srcset("dark", scene, format)}" sizes="${sizes}">`);
    }
  }
  for (const format of ["avif", "webp"]) {
    sources.push(`<source type="image/${format}" srcset="${srcset(fallbackTheme, scene, format)}" sizes="${sizes}">`);
  }
  const extra = Object.entries(attrs)
    .filter(([name]) => name.startsWith("data-") || name === "class")
    .map(([name, value]) => ` ${name}="${value}"`)
    .join("");
  return `<picture${extra}>${sources.join("")}<img src="/src/assets/screenshots/${fallbackTheme}/${scene}-1440.webp" width="${SHOT_RATIO.width}" height="${SHOT_RATIO.height}" alt="${alt}" ${eager ? 'fetchpriority="high"' : 'loading="lazy"'} decoding="async"></picture>`;
}

/** Inlines a lucide icon (the app's icon set) without shipping React to the site. */
function icon(name: string) {
  const source = readFileSync(path.join(repoRoot, "node_modules/lucide-react/dist/esm/icons", `${name}.js`), "utf8");
  const nodes = [...source.matchAll(/\[\s*"(\w+)",\s*\{([^}]*)\}\s*\]/g)].map(([, element, body]) => {
    const attrs = [...body!.matchAll(/(\w+): "([^"]*)"/g)]
      .filter(([, key]) => key !== "key")
      .map(([, key, value]) => `${key}="${value}"`)
      .join(" ");
    return `<${element} ${attrs}/>`;
  });
  if (nodes.length === 0) throw new Error(`Unknown lucide icon: ${name}`);
  return `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">${nodes.join("")}</svg>`;
}

/**
 * Social cards need absolute URLs, so `SITE_URL` (the deployed origin and
 * path, e.g. https://oa-labs.github.io/dispatch/) is used when it is known.
 */
function socialUrl(file: string) {
  const siteUrl = process.env.SITE_URL;
  return siteUrl ? new URL(file, siteUrl.endsWith("/") ? siteUrl : `${siteUrl}/`).href : `${siteBase}${file}`;
}

function siteMarkup(): Plugin {
  return {
    name: "threestrands-site-markup",
    transformIndexHtml: {
      order: "pre",
      handler: (html) =>
        html
          .replaceAll("%OG_IMAGE%", socialUrl("og.png"))
          .replaceAll("%SITE_URL%", socialUrl(""))
          .replace(/<ts-shot\b([^>]*)><\/ts-shot>/g, (_, attrs: string) => shot(attributes(attrs)))
          .replace(/<ts-icon name="([\w-]+)"><\/ts-icon>/g, (_, name: string) => icon(name))
          .replaceAll("%APP_VERSION%", version),
    },
  };
}

export default defineConfig({
  root: siteRoot,
  base: siteBase,
  publicDir: path.join(siteRoot, "public"),
  plugins: [siteMarkup()],
  server: { port: 1423, strictPort: true },
  preview: { port: 1424, strictPort: true },
  build: {
    outDir: path.join(siteRoot, "dist"),
    emptyOutDir: true,
    target: "es2022",
    assetsInlineLimit: 0,
    rollupOptions: {
      input: {
        index: path.join(siteRoot, "index.html"),
        privacy: path.join(siteRoot, "privacy.html"),
      },
    },
  },
});
