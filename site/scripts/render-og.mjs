// Renders site/og.html to the committed 1200×630 social card, site/public/og.png:
//
//   pnpm site:og
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";
import { createServer } from "vite";

const siteRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const server = await createServer({
  configFile: path.join(siteRoot, "vite.config.ts"),
  server: { port: 0, strictPort: false },
  logLevel: "error",
});
await server.listen();
const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: { width: 1200, height: 630 }, deviceScaleFactor: 1 });
  await page.goto(new URL("og.html", server.resolvedUrls.local[0]).href);
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all([...document.images].map((image) => image.decode()));
  });
  const output = path.join(siteRoot, "public", "og.png");
  await page.screenshot({ path: output });
  console.log(`Wrote ${path.relative(process.cwd(), output)}`);
} finally {
  await browser.close();
  await server.close();
}
