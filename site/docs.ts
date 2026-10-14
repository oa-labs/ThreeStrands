import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Marked, type Tokens } from "marked";
import type { Plugin } from "vite";
import { shortcutDisplaySteps, shortcutGroups } from "../src/shortcutCatalog";

const siteRoot = path.dirname(fileURLToPath(import.meta.url));
const docsRoot = path.join(siteRoot, "docs");

/**
 * Docs pages in sidebar order. Each slug has a `docs/<slug>.md` source and is
 * published at `docs/<slug>.html`.
 */
export const DOC_SLUGS = [
  "index",
  "getting-started",
  "triage",
  "compose",
  "tasks-and-goals",
  "calendar",
  "contacts",
  "ai",
  "settings-and-sync",
  "keyboard-shortcuts",
] as const;

export type DocSlug = (typeof DOC_SLUGS)[number];

type DocMeta = { title: string; nav: string; summary: string };
type Heading = { id: string; text: string };
type RenderedDoc = DocMeta & { slug: DocSlug; html: string; headings: Heading[] };

/** Reads the `key: value` front matter that opens every docs page. */
export function parseFrontMatter(source: string): { meta: Record<string, string>; body: string } {
  const match = /^---\n([\s\S]*?)\n---\n/.exec(source);
  if (!match) throw new Error("Docs pages must start with a --- front matter block");
  const meta: Record<string, string> = {};
  for (const line of match[1]!.split("\n")) {
    const separator = line.indexOf(":");
    if (separator < 1) throw new Error(`Malformed front matter line: ${line}`);
    meta[line.slice(0, separator).trim()] = line.slice(separator + 1).trim();
  }
  return { meta, body: source.slice(match[0].length) };
}

export function headingId(text: string): string {
  return text
    .toLowerCase()
    .replace(/<[^>]+>/g, "")
    .replace(/&[a-z]+;/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
}

export function escapeHtml(text: string): string {
  return text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

/** The app's shortcut catalog as one table per group, replacing `<ts-shortcuts></ts-shortcuts>`. */
export function renderShortcutTables(): string {
  return shortcutGroups()
    .map(({ group, entries }) => {
      const rows = entries
        .map((entry) => {
          const keys = entry.keys
            .map((key) => `<span class="docs-keys">${shortcutDisplaySteps(key).map((step) => `<kbd>${escapeHtml(step)}</kbd>`).join(" <small>then</small> ")}</span>`)
            .join(" <small>or</small> ");
          return `<tr><th scope="row">${escapeHtml(entry.title)}</th><td>${keys}</td></tr>`;
        })
        .join("");
      return `<h2 id="${headingId(group)}">${group}</h2><table class="docs-shortcuts"><thead><tr><th scope="col">Action</th><th scope="col">Shortcut</th></tr></thead><tbody>${rows}</tbody></table>`;
    })
    .join("\n");
}

export function renderDoc(slug: DocSlug, source = readFileSync(path.join(docsRoot, `${slug}.md`), "utf8")): RenderedDoc {
  const { meta, body } = parseFrontMatter(source);
  for (const key of ["title", "nav", "summary"] as const) {
    if (!meta[key]) throw new Error(`docs/${slug}.md is missing "${key}" front matter`);
  }
  const marked = new Marked({
    gfm: true,
    renderer: {
      heading({ tokens, depth }: Tokens.Heading) {
        if (depth === 1) throw new Error(`docs/${slug}.md: the page title comes from front matter; start headings at ##`);
        const text = this.parser.parseInline(tokens);
        return `<h${depth} id="${headingId(text)}">${text}</h${depth}>\n`;
      },
      link({ href, title, tokens }: Tokens.Link) {
        const text = this.parser.parseInline(tokens);
        // External links never hand the opener to another site.
        const rel = /^https?:/.test(href) ? ' rel="noopener"' : "";
        return `<a href="${escapeHtml(href)}"${title ? ` title="${escapeHtml(title)}"` : ""}${rel}>${text}</a>`;
      },
    },
  });
  const html = marked.parse(body.replace("<ts-shortcuts></ts-shortcuts>", () => `\n${renderShortcutTables()}\n`), { async: false });
  const headings = [...html.matchAll(/<h2 id="([^"]+)">(.*?)<\/h2>/g)].map(([, id, text]) => ({ id: id!, text: text! }));
  const duplicate = headings.find((heading, index) => headings.findIndex((other) => other.id === heading.id) !== index);
  if (duplicate) throw new Error(`docs/${slug}.md has two sections with the id "${duplicate.id}"`);
  return { slug, title: meta.title!, nav: meta.nav!, summary: meta.summary!, html, headings };
}

export const BRAND_MARK = `<svg class="brand-mark" viewBox="80 80 864 864" aria-hidden="true" focusable="false">
            <rect x="80" y="80" width="864" height="864" rx="196" fill="#333183" />
            <g fill="none" stroke-linecap="round" transform="translate(80 80) scale(.84375)">
              <circle cx="512" cy="512" r="372" stroke="#f3eae9" stroke-width="48" />
              <circle cx="512" cy="512" r="322" stroke="#f3eae9" stroke-width="28" />
              <path d="M 340 380 Q 426 286 512 380 T 684 380" stroke="#9f8bcb" stroke-width="42" />
              <path d="M 340 512 Q 426 418 512 512 T 684 512" stroke="#f3eae9" stroke-width="42" />
              <path d="M 340 644 Q 426 550 512 644 T 684 644" stroke="#fdad81" stroke-width="42" />
            </g>
          </svg>`;

function sidebar(current: RenderedDoc, pages: RenderedDoc[]): string {
  const items = pages
    .map((page) => {
      if (page.slug !== current.slug) return `<li><a href="${page.slug}.html">${escapeHtml(page.nav)}</a></li>`;
      const sections = current.headings.length
        ? `<ol class="docs-nav-sections">${current.headings.map((heading) => `<li><a href="#${heading.id}">${heading.text}</a></li>`).join("")}</ol>`
        : "";
      return `<li><a href="${page.slug}.html" aria-current="page">${escapeHtml(page.nav)}</a>${sections}</li>`;
    })
    .join("");
  return `<nav class="docs-nav" aria-label="Documentation"><p class="legal-toc-title">Docs</p><ol>${items}</ol></nav>`;
}

function pager(current: RenderedDoc, pages: RenderedDoc[]): string {
  const index = pages.findIndex((page) => page.slug === current.slug);
  const previous = pages[index - 1];
  const next = pages[index + 1];
  const link = (page: RenderedDoc, rel: "prev" | "next") =>
    `<a class="docs-pager-${rel}" href="${page.slug}.html" rel="${rel}"><small>${rel === "prev" ? "Previous" : "Next"}</small>${escapeHtml(page.nav)}</a>`;
  return `<nav class="docs-pager" aria-label="More docs">${previous ? link(previous, "prev") : "<span></span>"}${next ? link(next, "next") : ""}</nav>`;
}

export function renderDocPage(slug: DocSlug): string {
  const pages = DOC_SLUGS.map((candidate) => renderDoc(candidate));
  const doc = pages.find((page) => page.slug === slug)!;
  const pageTitle = slug === "index" ? "ThreeStrands Docs" : `${doc.title} — ThreeStrands Docs`;
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>${escapeHtml(pageTitle)}</title>
    <meta name="description" content="${escapeHtml(doc.summary)}" />
    <meta name="theme-color" content="#1f1b5c" />
    <meta name="color-scheme" content="light dark" />
    <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
    <link rel="apple-touch-icon" href="/apple-touch-icon.png" />
    <meta property="og:type" content="article" />
    <meta property="og:title" content="${escapeHtml(pageTitle)}" />
    <meta property="og:description" content="${escapeHtml(doc.summary)}" />
    <meta property="og:url" content="%SITE_URL%docs/${slug}.html" />
    <meta property="og:image" content="%OG_IMAGE%" />
    <meta name="twitter:card" content="summary_large_image" />
    <script type="module" src="/src/legal.ts"></script>
  </head>
  <body>
    <a class="skip-link" href="#main">Skip to Content</a>

    <header class="nav" data-nav>
      <div class="nav-inner">
        <a class="brand" href="../" aria-label="ThreeStrands home">
          ${BRAND_MARK}
          <span class="brand-name">ThreeStrands</span>
        </a>
        <nav class="nav-links" aria-label="Sections">
          <a href="../#features">Features</a>
          <a href="../#privacy">Privacy</a>
          <a href="../#roadmap">Roadmap</a>
          <a href="index.html" aria-current="page">Docs</a>
        </nav>
        <a class="button button-small button-glow" href="https://github.com/oa-labs/ThreeStrands/releases/latest" rel="noopener" data-download>
          <ts-icon name="download"></ts-icon>Download
        </a>
      </div>
    </header>

    <main id="main" tabindex="-1">
      <section class="legal-hero docs-hero dusk" aria-labelledby="doc-title">
        <div class="container">
          <p class="eyebrow">Docs</p>
          <h1 id="doc-title" class="display-2">${escapeHtml(doc.title)}</h1>
          <p class="lead">${escapeHtml(doc.summary)}</p>
        </div>
      </section>

      <section class="legal paper" aria-label="${escapeHtml(doc.title)}">
        <div class="container legal-grid docs-grid">
          ${sidebar(doc, pages)}
          <article class="docs-body">
${doc.html}
            ${pager(doc, pages)}
          </article>
        </div>
      </section>
    </main>

    <footer class="footer dusk">
      <div class="container footer-inner">
        <p class="footer-brand"><span class="brand-name">ThreeStrands</span> <span>v%APP_VERSION%</span></p>
        <nav aria-label="Project">
          <a href="index.html">Docs</a>
          <a href="../changelog.html">Changelog</a>
          <a href="https://github.com/oa-labs/ThreeStrands" rel="noopener">GitHub</a>
          <a href="https://github.com/oa-labs/ThreeStrands/releases/latest" rel="noopener">Releases</a>
          <a href="https://github.com/oa-labs/ThreeStrands#readme" rel="noopener">README</a>
          <a href="https://github.com/oa-labs/ThreeStrands/blob/master/LICENSE" rel="noopener">License</a>
          <a href="../privacy.html">Privacy Policy</a>
          <a href="../terms.html">Terms of Service</a>
        </nav>
        <p class="footer-note">Made with love by <a href="https://www.openarc.net/custom-software/create-my-product" rel="noopener">OpenArc</a>. © 2026 OpenArc LLC. All rights reserved.</p>
      </div>
    </footer>
  </body>
</html>
`;
}

const docPath = (slug: DocSlug) => path.join(docsRoot, `${slug}.html`);

/** Rollup inputs for every docs page. The HTML is virtual; only the Markdown exists on disk. */
export function docInputs(): Record<string, string> {
  return Object.fromEntries(DOC_SLUGS.map((slug) => [`docs/${slug}`, docPath(slug)]));
}

/**
 * Serves `docs/<slug>.html` from `docs/<slug>.md` in both dev and build. The
 * generated HTML then goes through the same `transformIndexHtml` hooks as the
 * hand-written pages, so `<ts-shot>` and `<ts-icon>` work in Markdown.
 */
export function docsPages(): Plugin {
  const bySlug = new Map(DOC_SLUGS.map((slug) => [docPath(slug), slug]));
  return {
    name: "threestrands-docs-pages",
    resolveId(id) {
      return bySlug.has(id) ? id : null;
    },
    load(id) {
      const slug = bySlug.get(id);
      return slug ? renderDocPage(slug) : null;
    },
    configureServer(server) {
      server.watcher.add(path.join(docsRoot, "*.md"));
      server.middlewares.use(async (request, response, next) => {
        const pathname = new URL(request.url ?? "/", "http://localhost").pathname;
        const match = /\/docs\/(?:([\w-]+)\.html)?$/.exec(pathname);
        const slug = (match?.[1] ?? (match ? "index" : undefined)) as DocSlug | undefined;
        if (!slug || !DOC_SLUGS.includes(slug)) return next();
        try {
          const html = await server.transformIndexHtml(pathname, renderDocPage(slug), request.originalUrl);
          response.setHeader("Content-Type", "text/html");
          response.end(html);
        } catch (error) {
          next(error);
        }
      });
    },
  };
}
