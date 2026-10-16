import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { publishedSections, renderChangelogPage, sectionId } from "../../site/changelog";
import { DOC_SLUGS, headingId, parseFrontMatter, renderDoc } from "../../site/docs";
import { shortcutGroups } from "../../src/shortcutCatalog";

const RELEASES = "https://github.com/oa-labs/ThreeStrands/releases/latest";
const LICENSE_URL = "https://github.com/oa-labs/ThreeStrands/blob/master/LICENSE";
const TOUR_SCENES = ["inbox", "split-inbox", "newsletter", "reply", "tasks", "goals", "inbox-zero", "find-times", "calendar-week", "calendar-create", "contacts", "files"];
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
    await expect(privacy.getByRole("link", { name: "Inspect the source" })).toHaveAttribute("href", "https://github.com/oa-labs/ThreeStrands");
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
    await expect(tour).toContainText("Set goals for the year, half, and quarter");
    await expect(tour).toContainText("mark it achieved, drop it, or carry it forward");
    const roadmap = page.locator("#roadmap");
    await expect(roadmap.getByRole("heading", { level: 3 })).toHaveText([
      "Snooze", "Sending Aliases", "Drafts Across Devices",
      "Connected Address Books", "Smarter Inbox Rules", "Microsoft 365",
      "Encrypted Sync Reliability", "Shared and Mobile Workflows",
    ]);
    await expect(roadmap).toContainText("verified Gmail aliases");
    await expect(roadmap).toContainText("clear controls over draft synchronization");
    await expect(roadmap).toContainText("Google Contacts and CardDAV address books connected with ongoing synchronization");
    await expect(roadmap).toContainText("CSV and vCard import and export are available today");
    await expect(roadmap).toContainText("Gmail and IMAP/SMTP connections available today");
    await expect(roadmap).not.toContainText(/scheduled sending|Gmail is the only/i);
    await expect(roadmap).not.toContainText(/\bbeta\b/i);
  });

  test("promotes the release’s signatures, newsletter controls, scheduling, and inbox-zero workflows", async ({ page }) => {
    await page.goto("./");
    const writing = page.locator('[data-step="reply"]');
    for (const claim of ["formatted signatures with logos", "import your signature from Gmail", "Sent", "Edit as New"]) {
      await expect(writing).toContainText(claim);
    }
    const newsletter = page.locator('[data-step="newsletter"]');
    for (const claim of ["Unsubscribe", "Mute", "archives it automatically", "across your devices"]) {
      await expect(newsletter).toContainText(claim);
    }
    const times = page.locator('[data-step="find-times"]');
    for (const claim of ["optional Draft Assist", "next week", "lunch gap", "Drive Time Buffer"]) {
      await expect(times).toContainText(claim);
    }
    const zero = page.locator('[data-step="inbox-zero"]');
    for (const claim of ["open tasks", "Keep in Touch", "today’s agenda", "last checked"]) {
      await expect(zero).toContainText(claim);
    }
    await expect(page.locator(".privacy-shot")).toContainText("Trust checks run on your computer");
    await expect(page.locator(".privacy-shot")).toContainText("Show link takes you to the link in the email");
    await expect(page.locator('[data-step="files"]')).toContainText("On macOS, preview");
    await expect(page.locator('[data-step="files"]')).toContainText("never previewed");
  });

  test("describes mail compatibility separately from calendars in copy and metadata", async ({ page }) => {
    await page.goto("./");
    await expect(page).toHaveTitle(/Gmail and IMAP/);
    for (const selector of ['meta[name="description"]', 'meta[property="og:description"]']) {
      await expect(page.locator(selector)).toHaveAttribute("content", /Gmail and IMAP/);
    }
    await expect(page.locator(".hero .fine-print")).toContainText("Gmail + IMAP · macOS + Linux");
    const tour = page.locator("#features");
    await expect(tour).toContainText("Proton Mail through Proton Mail Bridge");
    await expect(tour).toContainText("Google Calendar connects separately and works alongside either mail provider");
    await expect(tour).toContainText("CalDAV and CardDAV connections are not yet available");
    await expect(tour.locator('[data-step="inbox"]')).toContainText("Gmail and IMAP accounts");
    await expect(page.locator(".flow")).toHaveAttribute("aria-label", /Gmail or your IMAP\/SMTP server/);
    await expect(page.locator(".finale .fine-print")).toContainText("Gmail and IMAP/SMTP");
    await expect(page.locator("main")).not.toContainText(/Gmail today|Gmail is the only|every Gmail account/i);
  });

  test("promotes finding mail attachments with their source context and local stars", async ({ page }) => {
    await page.goto("./");
    const files = page.locator('[data-tour-steps] [data-step="files"]');
    await expect(files.getByRole("heading", { level: 3 })).toHaveText("Find the File Without Hunting Down the Email");
    for (const claim of [
      "attachments from your synced mail", "including archived conversations", "Search by filename",
      "sender, file type, direction, account, and date", "Star the files you return to",
      "jump straight to the source conversation", "Stars stay on this computer", "do not keep an offline copy",
    ]) await expect(files).toContainText(claim);
    await expect(files.locator("kbd")).toHaveText("5");
    await expect(files.locator(".tour-inline img")).toHaveAttribute("alt", /Files view.*starred rollout image.*preview.*source conversation/);
  });

  test("keeps scheduled delivery ownership and missed-time requirements beside the feature", async ({ page }) => {
    await page.goto("./");
    const scheduled = page.locator("#send-later");
    await expect(scheduled.getByRole("heading")).toHaveText("Send Later");
    for (const claim of [
      "date, time, and timezone", "Reschedule or return to draft before sending starts",
      "With encrypted sync enabled, other computers can see the schedule and its last reported status",
      "Sends from the computer where you scheduled it", "ThreeStrands must be running",
      "awake, online, and authorized", "Missed times wait for your review", "another computer never takes over",
    ]) await expect(scheduled).toContainText(claim);
    await expect(page.locator('[data-step="reply"]')).toContainText("Send now includes a ten-second undo window");
    await expect(page.locator(".stamps")).toContainText("Missed scheduled sends wait for your review");
    await expect(page.locator("main")).not.toContainText(/every send gets|on every message/i);
  });

  test("presents local compose checks, relationships, and calendar management as shipped", async ({ page }) => {
    await page.goto("./");
    const compose = page.locator('[data-step="reply"]');
    await expect(compose).toContainText("recipient history, shared files, related tasks, meetings, and available times");
    await expect(compose).toContainText("Local checks flag missing attachments, likely address typos, account mismatches");
    await expect(compose).toContainText("group addresses exposed in To or Cc");
    await expect(compose).toContainText("Optional AI draft review");
    const contacts = page.locator('[data-step="contacts"]');
    for (const claim of ["groups", "birthdays", "Keep in Touch", "CSV or vCard import and export", "Merge selected profiles", "recipient suggestions"]) {
      await expect(contacts).toContainText(claim);
    }
    await expect(page.locator('[data-step="calendar-week"]')).toContainText("Customize calendar colors, show or hide accounts");
    await expect(page.locator('[data-step="calendar-create"]')).toContainText("Edit or delete events you manage");
    await expect(page.locator('[data-step="calendar-create"]')).toContainText("Yes, No, or Maybe");
    await expect(page.locator(".detail-grid")).toContainText("no AI required");
    await expect(page.locator(".detail-grid")).toContainText("On macOS, choose ThreeStrands as your default mail app");
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
    await expect(ai).toContainText("preview of the subject, written text, and optional goal");
    await expect(ai).toContainText("Recipient fields, separately stored quoted history, and attachments are excluded");
    await expect(ai).toContainText("Review suggested wording before applying it; revisions can be undone");
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
      await expect(stamp).toContainText("Mail, drafts, queued send");
    }
    const sync = stamps.locator("li").filter({ has: page.getByRole("heading", { name: "Encrypted Sync", exact: true }) });
    await expect(sync).toContainText("tasks, goals, saved contacts");
    await expect(sync).toContainText("Scheduled-send summaries share the account, subject, timing, sending computer, and last reported status");
    await expect(sync).toContainText("queued send payloads, and provider credentials stay on each device");
    await expect(sync).toContainText("Scheduled bodies, recipients, and attachments never sync");
    await expect(stamps.locator(".tag")).toHaveCount(0);
    await expect(stamps).not.toContainText(/\bbeta\b/i);
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
    // Hero, command palette, sandboxed reader, Trust, two AI scenes, and one inline image per tour step.
    expect(described).toHaveLength(6 + TOUR_SCENES.length);
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
    await expect(body.locator("#device")).toContainText("IMAP/SMTP passwords or app passwords");
    await expect(body.locator("#connections")).toContainText("IMAP reads and synchronizes your mail and folders; SMTP submits outgoing messages");
    await expect(body.locator("#connections")).toContainText("Bridge running on your computer");
    await expect(body.locator("#connections")).toContainText("IMAP does not grant calendar or address-book access");
    await expect(body.locator("#ai")).toContainText("draft subject, written text, and optional goal shown in the preview");
    await expect(body.locator("#ai")).toContainText("Recipient fields, separately stored quoted history, and attachments are excluded");
    await expect(body.locator("#ai")).toContainText("Checks for likely compose mistakes run locally without AI");
    const sync = body.locator("#sync");
    await expect(sync).toContainText("queued sends, scheduled-send summaries, or credentials");
    await expect(sync).toContainText("Mail, drafts, queued send payloads, and credentials never sync");
    await expect(sync).toContainText("sending account, subject, chosen time and timezone, owning computer, and last reported delivery status");
    await expect(sync).toContainText("Message bodies, recipients (including Bcc), and attachments are excluded");
    await expect(sync).toContainText("other computers show a read-only summary");
    await expect(sync).toContainText("A stale summary does not establish whether delivery succeeded or failed");
    await expect(sync).toContainText("CSV and vCard exports are unencrypted");
    await expect(sync).toContainText("Recipient-suggestion suppression preferences stay on this computer");
    for (const scope of ["gmail.modify", "calendar.readonly", "calendar.events"]) {
      await expect(body.locator("code", { hasText: scope })).toHaveCount(1);
    }
    await expect(body).not.toContainText("gmail.labels");
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
    for (const file of ["", ...LEGAL_PAGES.map((legal) => legal.file), "docs/index.html", "changelog.html"]) {
      await page.goto(`./${file}`);
      await expect(page.locator(".footer-note")).toContainText("Made with love by OpenArc. © 2026 OpenArc LLC. All rights reserved.");
      await expect(page.locator(".footer-note").getByRole("link", { name: "OpenArc", exact: true })).toHaveAttribute(
        "href",
        "https://www.openarc.net/custom-software/create-my-product",
      );
      await expect(page.locator("footer").getByRole("link", { name: "License", exact: true })).toHaveAttribute("href", LICENSE_URL);
      expect(await page.locator("html").innerHTML()).not.toMatch(/open[- ]source/i);
    }
  });
});

test.describe("docs", () => {
  test("every Markdown page is published, in sidebar order", () => {
    const sources = readdirSync(path.resolve("site/docs")).filter((file) => file.endsWith(".md")).map((file) => file.slice(0, -3));
    expect([...sources].sort()).toEqual([...DOC_SLUGS].sort());
    expect(DOC_SLUGS[0]).toBe("index");
  });

  test("the renderer requires front matter and keeps the page title out of the body", () => {
    expect(() => parseFrontMatter("## No front matter")).toThrow(/front matter/);
    expect(parseFrontMatter("---\ntitle: A: B\nnav: A\n---\nBody")).toEqual({ meta: { title: "A: B", nav: "A" }, body: "Body" });
    expect(() => renderDoc("triage", "---\ntitle: T\nnav: T\n---\nBody")).toThrow(/summary/);
    expect(() => renderDoc("triage", "---\ntitle: T\nnav: T\nsummary: S\n---\n# Title")).toThrow(/start headings at ##/);
    expect(() => renderDoc("triage", "---\ntitle: T\nnav: T\nsummary: S\n---\n## Same\n\n## Same")).toThrow(/two sections/);
    expect(headingId("Send, Undo, and <em>Send Later</em>")).toBe("send-undo-and-send-later");
  });

  for (const from of ["./", "./privacy.html", "./terms.html", "./changelog.html"]) {
    test(`${from} links to the docs from the nav and the footer`, async ({ page }) => {
      await page.goto(from);
      await page.getByRole("navigation", { name: "Sections" }).getByRole("link", { name: "Docs" }).click();
      await expect(page).toHaveURL(/docs\/index\.html$/);
      await expect(page.getByRole("heading", { level: 1 })).toHaveText("ThreeStrands Docs");
      await page.goto(from);
      await page.locator("footer").getByRole("link", { name: "Docs" }).click();
      await expect(page).toHaveURL(/docs\/index\.html$/);
    });
  }

  for (const slug of DOC_SLUGS) {
    test(`docs/${slug}: title, sidebar, section links, and every internal link resolve`, async ({ page, request }) => {
      const doc = renderDoc(slug);
      await page.goto(`./docs/${slug}.html`);
      await expect(page.getByRole("heading", { level: 1 })).toHaveText(doc.title);
      await expect(page.locator(".brand-mark")).toHaveCSS("width", "32px");
      const sidebar = page.getByRole("navigation", { name: "Documentation" });
      await expect(sidebar.locator('a[aria-current="page"]')).toHaveText(doc.nav);
      const sections = await sidebar.locator(".docs-nav-sections a").evaluateAll((links) => links.map((link) => link.getAttribute("href")!));
      const headings = await page.locator(".docs-body h2").evaluateAll((elements) => elements.map((element) => `#${element.id}`));
      expect(sections).toEqual(headings);
      expect(headings.length).toBeGreaterThan(0);

      const internal = await page.locator("a[href]").evaluateAll((links) =>
        links.map((link) => (link as HTMLAnchorElement).href).filter((href) => href.startsWith(location.origin)),
      );
      for (const href of new Set(internal.map((href) => href.split("#")[0]!))) {
        expect((await request.get(href)).status(), href).toBe(200);
      }
      for (const href of internal.filter((href) => href.includes("#") && href.startsWith(page.url().split("#")[0]!))) {
        const id = decodeURIComponent(href.split("#")[1]!);
        if (id === "main") continue;
        expect(await page.locator(`[id="${id}"]`).count(), href).toBe(1);
      }
      const unsafe = await page.locator('a[href^="http"]').evaluateAll((links) =>
        links.filter((link) => !(link.getAttribute("rel") ?? "").split(/\s+/).includes("noopener")).map((link) => link.outerHTML),
      );
      expect(unsafe).toEqual([]);
    });

    test(`docs/${slug} loads every screenshot and makes no third-party requests`, async ({ page, baseURL }) => {
      const origin = new URL(baseURL!).origin;
      const foreign: string[] = [];
      page.on("request", (request) => {
        const url = new URL(request.url());
        if (url.protocol.startsWith("http") && url.origin !== origin) foreign.push(request.url());
      });
      await page.goto(`./docs/${slug}.html`);
      await scrollThrough(page);
      await page.waitForLoadState("networkidle");
      expect(foreign).toEqual([]);
      await expect.poll(() => page.locator("img").evaluateAll((images) =>
        images.filter((image) => image.getClientRects().length > 0 && (!image.complete || image.naturalWidth === 0)).length,
      )).toBe(0);
      const undescribed = await page.locator(".docs-body img").evaluateAll((images) => images.filter((image) => !image.getAttribute("alt")).length);
      expect(undescribed).toBe(0);
    });
  }

  test("documents the current compose and scheduling controls and the limits of Trust", async ({ page }) => {
    await page.goto("./docs/compose.html");
    const writing = page.locator(".docs-body");
    for (const claim of ["Import from Gmail…", "Regenerate from Signature", "Send & archive", "Send options", "Sent and Edit as New", "Finished sends move to Sent"]) {
      await expect(writing).toContainText(claim);
    }
    await expect(writing).not.toContainText("Send & Mark Done");
    await page.goto("./docs/calendar.html");
    const calendar = page.locator(".docs-body");
    for (const claim of ["Drive Time Buffer", "Draft Reply With These Times", "also work without Draft Assist", "All day"]) {
      await expect(calendar).toContainText(claim);
    }
    await page.goto("./docs/triage.html");
    const triage = page.locator(".docs-body");
    for (const claim of ["Muted Senders", "Known sender", "not a promise that every message is safe", "never previewed", "Remote images remain blocked until you allow them"]) {
      await expect(triage).toContainText(claim);
    }
  });

  test("the shortcut reference lists exactly what the app's Keyboard Shortcuts window lists", async ({ page }) => {
    await page.goto("./docs/keyboard-shortcuts.html");
    for (const { group, entries } of shortcutGroups()) {
      const table = page.locator(`h2#${headingId(group)} + table`);
      await expect(table.locator("tbody th")).toHaveText(entries.map((entry) => entry.title));
    }
    const palette = page.locator("tbody tr", { has: page.locator("th", { hasText: /^Command Palette$/ }) });
    await expect(palette.locator("kbd")).toHaveText(["⌘/Ctrl + k"]);
    const inbox = page.locator("tbody tr", { has: page.locator("th", { hasText: /^Go to Inbox$/ }) });
    await expect(inbox.locator("kbd")).toHaveText(["g", "i"]);
    await expect(inbox.locator("small")).toHaveText("then");
  });

  test("docs load their assets when hosted under a repository path", async ({ page, baseURL }) => {
    const server = new URL(baseURL!);
    const mount = "/ThreeStrands/";
    await page.route(`${server.origin}/**`, async (route) => {
      const requested = new URL(route.request().url());
      if (!requested.pathname.startsWith(mount)) return route.abort();
      const asset = new URL(requested.pathname.slice(mount.length) + requested.search, server);
      await route.fulfill({ response: await route.fetch({ url: asset.href }) });
    });
    await page.goto(`${server.origin}${mount}docs/compose.html`);
    await expect(page.locator(".brand-mark")).toHaveCSS("width", "32px");
    await page.getByRole("link", { name: "ThreeStrands home" }).click();
    await expect(page.getByRole("heading", { level: 1 })).toContainText("Email at the Speed of");
    await page.unrouteAll({ behavior: "ignoreErrors" });
  });

  for (const slug of ["index", "keyboard-shortcuts", "compose"] as const) {
    for (const colorScheme of ["light", "dark"] as const) {
      test(`docs/${slug} has no serious or critical axe violations in ${colorScheme} mode`, async ({ page }) => {
        await page.emulateMedia({ colorScheme, reducedMotion: "reduce" });
        await page.goto(`./docs/${slug}.html`);
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
      test(`docs/${slug} never scrolls horizontally at ${width}px`, async ({ page }) => {
        await page.setViewportSize({ width, height: 900 });
        await page.goto(`./docs/${slug}.html`);
        const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
        expect(overflow).toBeLessThanOrEqual(0);
      });
    }
  }
});

test.describe("changelog", () => {
  const fixture = [
    "# Changelog", "", "Repository preamble.", "",
    "## Unreleased", "", "### New", "- Not shipped yet.", "",
    "## 1.2.0 — 2026-10-14", "", "### New", "- A **bold** feature with `code`.", "", "### Fixed", "- A fix.", "",
    "## Highlights: Early Days", "", "How it started.", "", "### Improved", "- Summary entry.", "",
  ].join("\n");

  test("renders released and summary sections, never Unreleased entries or the preamble", () => {
    const html = renderChangelogPage(fixture);
    expect(html).not.toContain("Not shipped yet");
    expect(html).not.toContain("Repository preamble");
    expect(html).toContain('<h2 id="v1-2-0">1.2.0</h2>');
    expect(html).toContain('<time datetime="2026-10-14">October 14, 2026</time>');
    expect(html).toContain("<li>A <strong>bold</strong> feature with <code>code</code>.</li>");
    expect(html).toContain('<h2 id="highlights-early-days">Highlights: Early Days</h2>');
    expect(html).toContain("<p>How it started.</p>");
    expect(() => renderChangelogPage(fixture.replace("### Fixed", "### Changes"))).toThrow(/groups are New, Improved, Fixed/);
  });

  test("lists every published section in the sidebar, each jumping to its heading", async ({ page }) => {
    const sections = publishedSections();
    expect(sections.length).toBeGreaterThan(0);
    await page.goto("./changelog.html");
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("What's New");
    const targets = await page.getByRole("navigation", { name: "Releases" }).locator("a").evaluateAll((links) => links.map((link) => link.getAttribute("href")!));
    expect(targets).toEqual(sections.map((section) => `#${sectionId(section)}`));
    const headings = await page.locator(".changelog-body h2").evaluateAll((elements) => elements.map((element) => `#${element.id}`));
    expect(headings).toEqual(targets);
  });

  for (const from of ["./", "./privacy.html", "./terms.html", "./docs/index.html", "./changelog.html"]) {
    test(`${from} links to the changelog from its footer`, async ({ page }) => {
      await page.goto(from);
      await page.locator("footer").getByRole("link", { name: "Changelog" }).click();
      await expect(page).toHaveURL(/\/changelog\.html$/);
      await expect(page.locator("footer").getByRole("link", { name: "Changelog" })).toHaveAttribute("aria-current", "page");
    });
  }

  test("the roadmap points to what has already shipped", async ({ page }) => {
    await page.goto("./");
    await page.locator("#roadmap").getByRole("link", { name: /already shipped/ }).click();
    await expect(page).toHaveURL(/\/changelog\.html$/);
  });

  test("makes no third-party requests and keeps external links safe", async ({ page, baseURL }) => {
    const origin = new URL(baseURL!).origin;
    const foreign: string[] = [];
    page.on("request", (request) => {
      const url = new URL(request.url());
      if (url.protocol.startsWith("http") && url.origin !== origin) foreign.push(request.url());
    });
    await page.goto("./changelog.html");
    await page.waitForLoadState("networkidle");
    expect(foreign).toEqual([]);
    const unsafe = await page.locator('a[href^="http"]').evaluateAll((links) =>
      links.filter((link) => !(link.getAttribute("rel") ?? "").split(/\s+/).includes("noopener")).map((link) => link.outerHTML),
    );
    expect(unsafe).toEqual([]);
  });

  for (const colorScheme of ["light", "dark"] as const) {
    test(`has no serious or critical axe violations in ${colorScheme} mode`, async ({ page }) => {
      await page.emulateMedia({ colorScheme, reducedMotion: "reduce" });
      await page.goto("./changelog.html");
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
    test(`never scrolls horizontally at ${width}px`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.goto("./changelog.html");
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(overflow).toBeLessThanOrEqual(0);
    });
  }
});
