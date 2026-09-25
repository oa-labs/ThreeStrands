import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";

const RELEASES = "https://github.com/oa-labs/dispatch/releases/latest";
const TOUR_SCENES = ["inbox", "split-inbox", "reply", "tasks", "today-schedule", "calendar-week", "contacts"];
const { version } = JSON.parse(readFileSync(path.resolve("package.json"), "utf8")) as { version: string };

/** Walks the whole page so every lazy image and scroll-driven effect gets a chance to run. */
async function scrollThrough(page: Page) {
  await page.evaluate(async () => {
    const step = Math.max(200, Math.floor(window.innerHeight * 0.6));
    for (let y = 0; y < document.documentElement.scrollHeight; y += step) {
      window.scrollTo(0, y);
      await new Promise((resolve) => setTimeout(resolve, 60));
    }
    window.scrollTo(0, document.documentElement.scrollHeight);
  });
}

test.describe("links", () => {
  test("every download button points to the latest GitHub release", async ({ page }) => {
    await page.goto("./");
    const downloads = page.locator("a[data-download]");
    expect(await downloads.count()).toBeGreaterThanOrEqual(3);
    for (const href of await downloads.evaluateAll((links) => links.map((link) => link.getAttribute("href")))) {
      expect(href).toBe(RELEASES);
    }
    await expect(page.getByRole("link", { name: "Download on GitHub" }).first()).toBeVisible();
  });

  test("external links never hand the opener to another site", async ({ page }) => {
    await page.goto("./");
    const unsafe = await page.locator('a[href^="http"]').evaluateAll((links) =>
      links.filter((link) => !(link.getAttribute("rel") ?? "").split(/\s+/).includes("noopener")).map((link) => link.outerHTML),
    );
    expect(unsafe).toEqual([]);
  });

  test("shows the app version from package.json", async ({ page }) => {
    await page.goto("./");
    await expect(page.locator(".badge")).toContainText(`v${version}`);
  });
});

test.describe("privacy", () => {
  test("makes no third-party requests, even after every lazy asset loads", async ({ page, baseURL }) => {
    const origin = new URL(baseURL!).origin;
    const foreign: string[] = [];
    page.on("request", (request) => {
      const url = new URL(request.url());
      if (url.protocol.startsWith("http") && url.origin !== origin) foreign.push(request.url());
    });
    await page.goto("./");
    await scrollThrough(page);
    await page.waitForLoadState("networkidle");
    expect(foreign).toEqual([]);
  });
});

test.describe("images", () => {
  test("every rendered screenshot loads and meaningful images are described", async ({ page }) => {
    await page.goto("./");
    await scrollThrough(page);
    const pending = () =>
      page.locator("img").evaluateAll((elements) =>
        elements.filter((image) => image.getClientRects().length > 0 && !(image as HTMLImageElement).complete).length,
      );
    await expect.poll(pending, { timeout: 20_000 }).toBe(0);
    const images = await page.locator("img").evaluateAll((elements) =>
      elements.map((image) => {
        const element = image as HTMLImageElement;
        return {
          alt: element.getAttribute("alt"),
          rendered: element.getClientRects().length > 0,
          loaded: element.complete && element.naturalWidth > 0,
          hidden: Boolean(element.closest('[aria-hidden="true"]')),
          src: element.currentSrc || element.src,
        };
      }),
    );
    expect(images.filter((image) => image.alt === null)).toEqual([]);
    expect(images.filter((image) => image.rendered && !image.loaded)).toEqual([]);
    const described = images.filter((image) => !image.hidden && image.alt);
    // Hero, command palette, sandboxed reader, and one inline image per tour step.
    expect(described).toHaveLength(3 + TOUR_SCENES.length);
  });

  test("each tour step has a matching sticky frame and an inline fallback", async ({ page }) => {
    await page.goto("./");
    for (const scene of TOUR_SCENES) {
      await expect(page.locator(`[data-tour-frames] picture[data-scene="${scene}"]`)).toHaveCount(1);
      const inline = page.locator(`.tour-step[data-step="${scene}"] .tour-inline img`);
      await expect(inline).toHaveCount(1);
      expect(await inline.getAttribute("alt")).toBeTruthy();
    }
  });

  test("screenshots follow the visitor's color scheme", async ({ page }) => {
    const image = page.locator(".palette-figure img");
    // Hashed asset names drop the theme folder, so compare against the <source> the browser should pick.
    const pickedDarkSource = () =>
      image.evaluate((element) => {
        const picture = element.closest("picture")!;
        const dark = [...picture.querySelectorAll("source[media]")].map((source) => (source as HTMLSourceElement).srcset).join(",");
        const current = new URL((element as HTMLImageElement).currentSrc).pathname;
        return current.length > 1 && dark.includes(current);
      });
    await page.emulateMedia({ colorScheme: "dark" });
    await page.goto("./");
    await image.scrollIntoViewIfNeeded();
    await expect.poll(pickedDarkSource).toBe(true);
    await page.emulateMedia({ colorScheme: "light" });
    await page.reload();
    await image.scrollIntoViewIfNeeded();
    await expect.poll(() => image.evaluate((element) => (element as HTMLImageElement).currentSrc)).not.toBe("");
    await expect.poll(pickedDarkSource).toBe(false);
  });
});

test.describe("accessibility", () => {
  for (const colorScheme of ["light", "dark"] as const) {
    test(`has no serious or critical axe violations in ${colorScheme} mode`, async ({ page }) => {
      await page.emulateMedia({ colorScheme, reducedMotion: "reduce" });
      await page.goto("./");
      await scrollThrough(page);
      await page.addScriptTag({ path: path.resolve("node_modules/axe-core/axe.min.js") });
      const violations = await page.evaluate(async () => {
        const axe = (window as unknown as { axe: { run: (context: Document) => Promise<{ violations: { id: string; impact: string; nodes: { target: string[] }[] }[] }> } }).axe;
        const result = await axe.run(document);
        return result.violations
          .filter((violation) => violation.impact === "serious" || violation.impact === "critical")
          .map((violation) => ({ id: violation.id, targets: violation.nodes.map((node) => node.target.join(" ")).slice(0, 5) }));
      });
      expect(violations).toEqual([]);
    });
  }

  test("the skip link moves focus to the main content", async ({ page }) => {
    await page.goto("./");
    await page.keyboard.press("Tab");
    await expect(page.getByRole("link", { name: "Skip to Content" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.locator("main")).toBeFocused();
  });

  test("keyboard users reach a download button within the first few stops", async ({ page }) => {
    await page.goto("./");
    for (let stop = 0; stop < 8; stop += 1) {
      await page.keyboard.press("Tab");
      if (await page.evaluate(() => document.activeElement?.hasAttribute("data-download"))) return;
    }
    throw new Error("No download button in the first eight tab stops");
  });
});

test.describe("layout", () => {
  for (const width of [360, 390, 768, 1440]) {
    test(`never scrolls horizontally at ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.goto("./");
      await scrollThrough(page);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(overflow).toBeLessThanOrEqual(0);
    });
  }
});

test.describe("motion", () => {
  test("reduced motion stops every infinite animation and draws strands fully", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto("./");
    await scrollThrough(page);
    const infinite = await page.evaluate(() =>
      document
        .getAnimations()
        .filter((animation) => animation.playState === "running" && animation.effect?.getTiming().iterations === Infinity)
        .map((animation) => (animation as CSSAnimation).animationName ?? "unknown"),
    );
    expect(infinite).toEqual([]);
    const offsets = await page.locator(".draw").evaluateAll((paths) => paths.map((path) => parseFloat(getComputedStyle(path).strokeDashoffset)));
    expect(offsets.length).toBeGreaterThan(0);
    expect(offsets.every((offset) => offset === 0)).toBe(true);
  });
});

test.describe("interactions", () => {
  test("the hero switch swaps the screenshot theme", async ({ page }) => {
    await page.goto("./");
    const hero = page.locator("[data-hero-window]");
    await expect(hero).toHaveAttribute("data-theme-shot", "dark");
    await page.getByRole("button", { name: "Light" }).click();
    await expect(hero).toHaveAttribute("data-theme-shot", "light");
    await expect(page.getByRole("button", { name: "Light" })).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByRole("button", { name: "Dark" })).toHaveAttribute("aria-pressed", "false");
  });

  test("the shortcut demo responds to keys while it is on screen", async ({ page }) => {
    await page.goto("./");
    const demo = page.locator("[data-demo]");
    const caption = page.locator("[data-demo-caption]");
    const rows = page.locator("[data-demo-list] li");
    await demo.scrollIntoViewIfNeeded();
    await expect(rows).toHaveCount(5);

    await page.keyboard.press("j");
    await expect(caption).toContainText("Next conversation");
    await expect(rows.nth(1)).toHaveClass(/is-selected/);

    await page.keyboard.press("s");
    await expect(caption).toContainText("Starred");

    await page.keyboard.press("e");
    await expect(caption).toContainText("Archived");
    await expect(rows).toHaveCount(4);

    await page.getByRole("button", { name: /Command palette/ }).click();
    await expect(page.locator("[data-demo-palette]")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.locator("[data-demo-palette]")).toBeHidden();
  });

  test("the shortcut demo ignores keys typed into editable fields", async ({ page }) => {
    await page.goto("./");
    const demo = page.locator("[data-demo]");
    await demo.scrollIntoViewIfNeeded();
    const caption = page.locator("[data-demo-caption]");
    const before = await caption.textContent();
    await demo.evaluate((element) => {
      const input = document.createElement("input");
      input.setAttribute("aria-label", "Scratch field");
      element.prepend(input);
    });
    await page.getByRole("textbox", { name: "Scratch field" }).press("e");
    await expect(page.getByRole("textbox", { name: "Scratch field" })).toHaveValue("e");
    await expect(caption).toHaveText(before!);
    await expect(page.locator("[data-demo-list] li")).toHaveCount(5);
  });

  test("the tour window follows the step in the middle of the viewport", async ({ page }) => {
    await page.goto("./");
    for (const scene of TOUR_SCENES) {
      await page.locator(`.tour-step[data-step="${scene}"]`).evaluate((step) => step.scrollIntoView({ block: "center" }));
      await expect(page.locator(`[data-tour-frames] picture[data-scene="${scene}"]`)).toHaveAttribute("data-active", "");
      await expect(page.locator(`.tour-step[data-step="${scene}"]`)).toHaveAttribute("data-active", "");
    }
  });
});

test.describe("without JavaScript", () => {
  test.use({ javaScriptEnabled: false });

  test("still presents the full page", async ({ page }) => {
    await page.goto("./");
    await expect(page.getByRole("heading", { level: 1 })).toContainText("Email at the Speed of");
    await expect(page.locator("a[data-download]").first()).toBeVisible();
    await expect(page.locator("[data-theme-switch]")).toBeHidden();
    const step = page.locator('.tour-step[data-step="tasks"]');
    await expect.poll(() => step.evaluate((element) => getComputedStyle(element).opacity)).toBe("1");
  });
});
