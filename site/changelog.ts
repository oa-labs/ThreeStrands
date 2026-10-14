import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Marked, type Tokens } from "marked";
import type { Plugin } from "vite";
import { parseChangelog, type ChangelogSection } from "../scripts/changelog.mjs";
import { BRAND_MARK, escapeHtml, headingId } from "./docs";

const siteRoot = path.dirname(fileURLToPath(import.meta.url));
const changelogSource = path.join(siteRoot, "..", "CHANGELOG.md");
/** Virtual Rollup input; only CHANGELOG.md exists on disk. */
export const CHANGELOG_PAGE = path.join(siteRoot, "changelog.html");

type PublishedSection = Exclude<ChangelogSection, { kind: "unreleased" }>;

const inline = new Marked({
  gfm: true,
  renderer: {
    link({ href, title, tokens }: Tokens.Link) {
      const text = this.parser.parseInline(tokens);
      const rel = /^https?:/.test(href) ? ' rel="noopener"' : "";
      return `<a href="${escapeHtml(href)}"${title ? ` title="${escapeHtml(title)}"` : ""}${rel}>${text}</a>`;
    },
  },
});
const markdown = (text: string) => inline.parseInline(text, { async: false });

/** Release anchors stay stable as titles change, so links like #v0-116-0 keep working. */
export function sectionId(section: PublishedSection): string {
  return section.kind === "release" ? `v${section.version.replace(/[^0-9a-z]+/gi, "-")}` : headingId(section.title);
}

function releaseDate(date: string): string {
  return new Intl.DateTimeFormat("en-US", { dateStyle: "long", timeZone: "UTC" }).format(new Date(`${date}T00:00:00Z`));
}

/**
 * Published sections in file order. Unreleased entries stay off the site
 * until `pnpm release` stamps them with a version.
 */
export function publishedSections(source = readFileSync(changelogSource, "utf8")): PublishedSection[] {
  return parseChangelog(source).sections.filter((section): section is PublishedSection => section.kind !== "unreleased");
}

function renderSection(section: PublishedSection): string {
  const id = sectionId(section);
  const parts = [`<h2 id="${id}">${escapeHtml(section.kind === "release" ? section.version : section.title)}</h2>`];
  if (section.kind === "release") {
    parts.push(`<p class="changelog-date"><time datetime="${section.date}">${releaseDate(section.date)}</time></p>`);
  }
  const intro = section.intro.join("\n").split(/\n\s*\n/).map((paragraph) => paragraph.trim()).filter(Boolean);
  for (const paragraph of intro) parts.push(`<p>${markdown(paragraph)}</p>`);
  for (const group of section.groups) {
    parts.push(`<h3 id="${id}-${headingId(group.name)}">${group.name}</h3>`);
    parts.push(`<ul>${group.items.map((item) => `<li>${markdown(item.text)}</li>`).join("")}</ul>`);
  }
  return parts.join("\n");
}

export function renderChangelogPage(source?: string): string {
  const sections = publishedSections(source);
  const toc = sections
    .map((section) => `<li><a href="#${sectionId(section)}">${escapeHtml(section.kind === "release" ? section.version : section.title)}</a></li>`)
    .join("");
  const summary = "What's new in each ThreeStrands release, in plain language.";
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Changelog — ThreeStrands</title>
    <meta name="description" content="${summary}" />
    <meta name="theme-color" content="#1f1b5c" />
    <meta name="color-scheme" content="light dark" />
    <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
    <link rel="apple-touch-icon" href="/apple-touch-icon.png" />
    <meta property="og:type" content="website" />
    <meta property="og:title" content="ThreeStrands Changelog" />
    <meta property="og:description" content="${summary}" />
    <meta property="og:url" content="%SITE_URL%changelog.html" />
    <meta property="og:image" content="%OG_IMAGE%" />
    <meta name="twitter:card" content="summary_large_image" />
    <script type="module" src="/src/legal.ts"></script>
  </head>
  <body>
    <a class="skip-link" href="#main">Skip to Content</a>

    <header class="nav" data-nav>
      <div class="nav-inner">
        <a class="brand" href="./" aria-label="ThreeStrands home">
          ${BRAND_MARK}
          <span class="brand-name">ThreeStrands</span>
        </a>
        <nav class="nav-links" aria-label="Sections">
          <a href="./#features">Features</a>
          <a href="./#privacy">Privacy</a>
          <a href="./#roadmap">Roadmap</a>
          <a href="docs/index.html">Docs</a>
        </nav>
        <a class="button button-small button-glow" href="https://github.com/oa-labs/dispatch/releases/latest" rel="noopener" data-download>
          <ts-icon name="download"></ts-icon>Download
        </a>
      </div>
    </header>

    <main id="main" tabindex="-1">
      <section class="legal-hero docs-hero dusk" aria-labelledby="changelog-title">
        <div class="container">
          <p class="eyebrow">Changelog</p>
          <h1 id="changelog-title" class="display-2">What's <em>New</em></h1>
          <p class="lead">${summary}</p>
        </div>
      </section>

      <section class="legal paper" aria-label="Release history">
        <div class="container legal-grid docs-grid">
          <nav class="docs-nav changelog-nav" aria-label="Releases"><p class="legal-toc-title">Releases</p><ol>${toc}</ol></nav>
          <article class="docs-body changelog-body">
${sections.map(renderSection).join("\n")}
          </article>
        </div>
      </section>
    </main>

    <footer class="footer dusk">
      <div class="container footer-inner">
        <p class="footer-brand"><span class="brand-name">ThreeStrands</span> <span>v%APP_VERSION%</span></p>
        <nav aria-label="Project">
          <a href="docs/index.html">Docs</a>
          <a href="changelog.html" aria-current="page">Changelog</a>
          <a href="https://github.com/oa-labs/dispatch" rel="noopener">GitHub</a>
          <a href="https://github.com/oa-labs/dispatch/releases/latest" rel="noopener">Releases</a>
          <a href="https://github.com/oa-labs/dispatch#readme" rel="noopener">README</a>
          <a href="https://github.com/oa-labs/dispatch/blob/master/LICENSE" rel="noopener">License</a>
          <a href="privacy.html">Privacy Policy</a>
          <a href="terms.html">Terms of Service</a>
        </nav>
        <p class="footer-note">Made with love by <a href="https://www.openarc.net/custom-software/create-my-product" rel="noopener">OpenArc</a>. © 2026 OpenArc LLC. All rights reserved.</p>
      </div>
    </footer>
  </body>
</html>
`;
}

/** Serves `changelog.html` from CHANGELOG.md in dev and build, through the same HTML hooks as other pages. */
export function changelogPage(): Plugin {
  return {
    name: "threestrands-changelog-page",
    resolveId(id) {
      return id === CHANGELOG_PAGE ? id : null;
    },
    load(id) {
      return id === CHANGELOG_PAGE ? renderChangelogPage() : null;
    },
    configureServer(server) {
      server.watcher.add(changelogSource);
      server.middlewares.use(async (request, response, next) => {
        const pathname = new URL(request.url ?? "/", "http://localhost").pathname;
        if (!pathname.endsWith("/changelog.html")) return next();
        try {
          const html = await server.transformIndexHtml(pathname, renderChangelogPage(), request.originalUrl);
          response.setHeader("Content-Type", "text/html");
          response.end(html);
        } catch (error) {
          next(error);
        }
      });
    },
  };
}
