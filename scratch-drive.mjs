import { chromium } from "@playwright/test";

const outDir = "/private/tmp/claude-501/-Users-jreed-Source-dispatch/f56e7fc4-a358-49ee-b622-f23b8e7d894c/scratchpad";

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
page.on("console", (msg) => { if (msg.type() === "error") console.log("CONSOLE ERROR:", msg.text()); });
page.on("pageerror", (err) => console.log("PAGE ERROR:", err));

await page.goto("http://localhost:1420/");
await page.waitForTimeout(1500);
await page.screenshot({ path: `${outDir}/1-initial.png` });

// find a thread row and its checkbox
const rows = page.locator(".thread-list [role='option'], .thread-list .thread-row");
const count = await rows.count();
console.log("row count", count);

// Try hovering first row to reveal checkbox, then click it
const firstRow = rows.first();
await firstRow.hover();
await page.waitForTimeout(300);
await page.screenshot({ path: `${outDir}/2-hover-first.png` });

const rowCheck = firstRow.locator(".row-check");
await rowCheck.click({ force: true });
await page.waitForTimeout(300);
await page.screenshot({ path: `${outDir}/3-selected-first.png` });

// Now select a later row too, to confirm no shift
const laterRow = rows.nth(2);
await laterRow.hover();
await page.waitForTimeout(200);
await laterRow.locator(".row-check").click({ force: true });
await page.waitForTimeout(300);
await page.screenshot({ path: `${outDir}/4-selected-two.png` });

// Stress-test: force a very narrow inbox column + larger font-scale to
// reproduce the original overflow/wrap conditions, overriding inline styles.
await page.addStyleTag({
  content: `
    #inbox-panel { width: 280px !important; flex: 0 0 280px !important; }
    :root { --font-scale: 1.4 !important; }
  `,
});
await page.waitForTimeout(300);
await page.screenshot({ path: `${outDir}/5-narrow.png` });

await browser.close();
