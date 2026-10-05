import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";

const RELEASES = "https://github.com/oa-labs/dispatch/releases/latest";
const LICENSE_URL = "https://github.com/oa-labs/dispatch/blob/master/LICENSE";
const TOUR_SCENES = ["inbox", "split-inbox", "reply", "tasks", "today-schedule", "calendar-week", "calendar-create", "contacts"];
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

test.describe("feature claims", () => {
  test("explains source availability and the restriction on competing products", async ({ page }) => {
    await page.goto("./");
    await expect(page.locator(".hero .fine-print")).toContainText("Source available");
    await expect(page.locator('meta[name="description"]')).toHaveAttribute("content", /source-available/);
    await expect(page.locator('meta[property="og:description"]')).toHaveAttribute("content", /source-available/);
    const privacy = page.locator("#privacy");
    await expect(privacy.getByRole("link", { name: "Inspect the source" })).toHaveAttribute("href", "https://github.com/oa-labs/dispatch");
    await expect(privacy.getByRole("link", { name: "PolyForm Perimeter License 1.0.1" })).toHaveAttribute("href", LICENSE_URL);
    await expect(privacy).toContainText("personal or internal business use");
    await expect(privacy).toContainText("prohibits providing others with a competing product built from this software, even for free");
  });

  test("presents contacts and event creation as shipped features", async ({ page }) => {
    await page.goto("./");
    const tour = page.locator("[data-tour-steps]");
    await expect(tour).toContainText("Browse, add, and edit contacts");
    await expect(tour).toContainText("Create events with invitees");
    await expect(tour).toContainText("Standalone tasks");
    const roadmap = page.locator("#roadmap");
    await expect(roadmap.getByRole("heading", { level: 3 })).toHaveText([
      "Snooze and Scheduled Sending", "Sending Aliases and Signatures", "Drafts Across Devices",
      "Connected Address Books", "Smarter Inbox Rules", "More Mail Providers",
      "Encrypted Sync Reliability", "Shared and Mobile Workflows",
    ]);
    await expect(roadmap).toContainText("verified Gmail aliases");
    await expect(roadmap).toContainText("clear controls over draft synchronization");
    await expect(roadmap).toContainText("Import existing Google Contacts");
  });

  test("explains AI activation, sharing choices, and reviewed actions", async ({ page }) => {
    await page.goto("./");
    const ai = page.locator(".ai");
    await expect(ai).toContainText("AI starts off. Set up your provider and key, then enable the features you want.");
    await expect(ai).toContainText("Proactive briefs are a separate opt-in");
    await expect(ai).toContainText("qualifying conversations are sent to your provider");
    await expect(ai).toContainText("Chat searches other mail only when you choose Search all mail. You select which readable attachments to share");
    await expect(ai).toContainText("Contact enrichment reviews correspondence with that person when you request it");
    await expect(ai).toContainText("You review and apply tasks, calendar events, contact changes, and drafts. You choose when to send.");
    await expect(ai).toContainText("estimated costs");
    await expect(ai).toContainText("optional fast model");
    for (const scene of ["thread-assist", "thread-chat"]) {
      const shot = ai.locator(`picture:has(img[src*="${scene}-"])`);
      await expect(shot).toHaveCount(1);
      expect(await shot.locator("img").getAttribute("alt")).toBeTruthy();
    }
  });

  test("includes saved contacts in encrypted export and sync without promising mail transfer", async ({ page }) => {
    await page.goto("./");
    const stamps = page.locator(".stamps");
    for (const title of ["Encrypted Settings Export", "Encrypted Sync"]) {
      const stamp = stamps.locator("li").filter({ has: page.getByRole("heading", { name: new RegExp(title) }) });
      await expect(stamp).toContainText("saved contacts");
      await expect(stamp).toContainText("Mail, drafts, queued sends");
    }
    await expect(stamps.locator(".tag")).toHaveText("Beta");
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
  for (const mount of ["/", "/ThreeStrands/"]) {
    test(`the same build loads its assets and bounds images when hosted at ${mount}`, async ({ page, baseURL }) => {
      const server = new URL(baseURL!);
      const outsideMount: string[] = [];
      // Mount the production output at either URL, as Pages does when a
      // custom domain and the repository URL serve the same artifact.
      await page.route(`${server.origin}/**`, async (route) => {
        const requested = new URL(route.request().url());
        if (!requested.pathname.startsWith(mount)) {
          outsideMount.push(requested.pathname);
          await route.abort();
          return;
        }
        const asset = new URL(requested.pathname.slice(mount.length) + requested.search, server);
        await route.fulfill({ response: await route.fetch({ url: asset.href }) });
      });
      await page.goto(`${server.origin}${mount}`);
      // A missing stylesheet leaves SVGs at their intrinsic (gigantic) size.
      await expect(page.locator(".brand-mark")).toHaveCSS("width", "32px");
      await expect(page.locator("[data-theme-switch]")).toBeVisible();
      await scrollThrough(page);
      await expect.poll(() => page.locator("img").evaluateAll((images) =>
        images.filter((image) => image.getClientRects().length > 0 && (!image.complete || image.naturalWidth === 0)).length,
      )).toBe(0);
      const oversized = await page.locator("img").evaluateAll((images) =>
        images.filter((image) => image.getBoundingClientRect().width > document.documentElement.clientWidth)
          .map((image) => image.currentSrc),
      );
      expect(oversized).toEqual([]);
      expect(outsideMount).toEqual([]);
    });
  }

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
    // Hero, command palette, sandboxed reader, two AI scenes, and one inline image per tour step.
    expect(described).toHaveLength(5 + TOUR_SCENES.length);
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

const LEGAL_PAGES = [
  { file: "privacy.html", link: "Privacy Policy" },
  { file: "terms.html", link: "Terms of Service" },
];

test.describe("legal pages", () => {
  for (const { file, link } of LEGAL_PAGES) {
    test(`${file} is linked from every page footer and links back home`, async ({ page }) => {
      for (const from of ["./", ...LEGAL_PAGES.map((other) => `./${other.file}`)]) {
        await page.goto(from);
        await page.locator("footer").getByRole("link", { name: link }).click();
        await expect(page).toHaveURL(new RegExp(`${file.replace(".", "\\.")}$`));
        await expect(page.getByRole("heading", { level: 1 })).toHaveText(link);
      }
      await expect(page.locator(".brand-mark")).toHaveCSS("width", "32px");
      await expect(page.locator("footer").getByRole("link", { name: link })).toHaveAttribute("aria-current", "page");
      await page.getByRole("link", { name: "ThreeStrands home" }).click();
      await expect(page.getByRole("heading", { level: 1 })).toContainText("Email at the Speed of");
    });

    test(`${file}: every contents entry jumps to a section on the page`, async ({ page }) => {
      await page.goto(`./${file}`);
      const targets = await page.locator(".legal-toc a").evaluateAll((links) => links.map((link) => link.getAttribute("href")!));
      expect(targets.length).toBeGreaterThan(10);
      const sections = await page.locator(".legal-body > section").evaluateAll((elements) => elements.map((element) => `#${element.id}`));
      expect(targets).toEqual(sections);
    });

    test(`${file} makes no third-party requests and keeps external links safe`, async ({ page, baseURL }) => {
      const origin = new URL(baseURL!).origin;
      const foreign: string[] = [];
      page.on("request", (request) => {
        const url = new URL(request.url());
        if (url.protocol.startsWith("http") && url.origin !== origin) foreign.push(request.url());
      });
      await page.goto(`./${file}`);
      await page.waitForLoadState("networkidle");
      expect(foreign).toEqual([]);
      const unsafe = await page.locator('a[href^="http"]').evaluateAll((links) =>
        links.filter((link) => !(link.getAttribute("rel") ?? "").split(/\s+/).includes("noopener")).map((link) => link.outerHTML),
      );
      expect(unsafe).toEqual([]);
    });

    for (const colorScheme of ["light", "dark"] as const) {
      test(`${file} has no serious or critical axe violations in ${colorScheme} mode`, async ({ page }) => {
        await page.emulateMedia({ colorScheme, reducedMotion: "reduce" });
        await page.goto(`./${file}`);
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

    for (const width of [360, 390, 768, 1440]) {
      test(`${file} never scrolls horizontally at ${width}px`, async ({ page }) => {
        await page.setViewportSize({ width, height: 900 });
        await page.goto(`./${file}`);
        const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
        expect(overflow).toBeLessThanOrEqual(0);
      });
    }
  }

  test("the privacy policy states the local-first data commitments", async ({ page }) => {
    await page.goto("./privacy.html");
    const body = page.locator(".legal-body");
    await expect(body).toContainText("There is no ThreeStrands account, backend, or relay");
    await expect(body).toContainText("does not send usage analytics, telemetry, or crash reports to us");
    await expect(body).toContainText("Google API Services User Data Policy, including the Limited Use requirements");
    await expect(body).toContainText("AI is off until you choose a provider");
    await expect(body).toContainText("Remote images in email are blocked by default");
    await expect(body).toContainText("Removing an account in the app does not revoke the grant with Google");
    for (const scope of ["gmail.modify", "gmail.labels", "calendar.readonly", "calendar.events"]) {
      await expect(body.locator("code", { hasText: scope })).toHaveCount(1);
    }
  });

  test("the terms license the app under Perimeter, reserve ownership, and retain liability protections", async ({ page }) => {
    await page.goto("./terms.html");
    const body = page.locator(".legal-body");
    const license = body.locator("#license");
    await expect(license).toContainText("ThreeStrands is source available under the PolyForm Perimeter License 1.0.1");
    await expect(body.locator("#license")).toContainText("Copyright © 2026 OpenArc LLC. All rights reserved.");
    await expect(license.getByRole("link", { name: "PolyForm Perimeter License 1.0.1" })).toHaveAttribute("href", LICENSE_URL);
    await expect(license).toContainText("You can inspect, build, use, and modify the software");
    await expect(license).toContainText("Copying and redistribution are allowed subject to the license's conditions");
    await expect(license).toContainText("prohibits providing others with a competing product built from this software, even if that product is free");
    await expect(license).toContainText("These Terms do not narrow permissions granted by the software license");
    await expect(license).not.toContainText("revocable");
    await expect(license).not.toContainText("Reverse engineer, decompile, or disassemble");
    await expect(license).not.toContainText("any purpose other than reviewing it");
    await expect(body.locator("#termination")).toContainText("Termination of these Terms does not itself terminate your rights under the PolyForm Perimeter License");
    await expect(body.locator("#termination")).toContainText("including its provisions for correcting violations");
    await expect(body.locator("#general")).toContainText("Updates to these Terms do not change permissions already granted under the software license");
    await expect(body.locator("#warranty .legal-caps")).toContainText("provided “as is” and “as available”");
    await expect(body.locator("#liability .legal-caps").first()).toContainText("will not be liable for any indirect");
    await expect(body.locator("#liability")).toContainText("fifty U.S. dollars (US$50)");
    await expect(body.locator("#law")).toContainText("laws of the Commonwealth of Pennsylvania");
    await expect(body.locator("#ai")).toContainText("AI output can be inaccurate");
    await expect(body.getByRole("link", { name: "Privacy Policy" })).toHaveAttribute("href", "privacy.html");
  });

  test("every page credits OpenArc, links to the license, and makes no open-source claim", async ({ page }) => {
    for (const file of ["", ...LEGAL_PAGES.map((legal) => legal.file)]) {
      await page.goto(`./${file}`);
      await expect(page.locator(".footer-note")).toContainText("© 2026 OpenArc LLC. All rights reserved.");
      await expect(page.locator("footer").getByRole("link", { name: "License", exact: true })).toHaveAttribute("href", LICENSE_URL);
      expect(await page.locator("html").innerHTML()).not.toMatch(/open[- ]source/i);
    }
  });
});
