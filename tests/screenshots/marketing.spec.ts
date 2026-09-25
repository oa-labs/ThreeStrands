import path from "node:path";
import { expect, test, type Page } from "@playwright/test";

/**
 * Captures marketing screenshots of the browser preview running the
 * fictional showcase dataset (`src/data/showcaseDataset.ts`). Run with
 * `pnpm screenshots`; images land in `artifacts/screenshots/<theme>/`.
 *
 * SCREENSHOT_DIR overrides the output folder and SCREENSHOT_NOW (any
 * `Date`-parseable string) overrides the frozen clock, which defaults to a
 * Tuesday mid-morning so the inbox, today's schedule, and week view are full.
 */
const outputDir = process.env.SCREENSHOT_DIR ?? path.join("artifacts", "screenshots");
const frozenNow = new Date(process.env.SCREENSHOT_NOW ?? "2026-09-22T10:24:00-07:00");

type Scene = { name: string; capture: (page: Page) => Promise<void> };

async function waitForMessage(page: Page) {
  const frame = page.getByTitle("Message content");
  await expect(frame).toBeVisible();
  await expect.poll(() => frame.evaluate((element) => (element as HTMLIFrameElement).contentDocument?.body?.textContent?.trim().length ?? 0)).toBeGreaterThan(0);
}

async function openThread(page: Page, subject: RegExp) {
  await page.getByRole("option", { name: subject }).click();
  await expect(page.getByRole("heading", { name: subject })).toBeVisible();
  await waitForMessage(page);
}

const scenes: Scene[] = [
  {
    name: "inbox",
    capture: async (page) => {
      await openThread(page, /Q4 launch plan — final review/);
    },
  },
  {
    name: "split-inbox",
    capture: async (page) => {
      await page.getByRole("radio", { name: /^Maya Chen/ }).first().click();
      await page.getByRole("tab", { name: /Notifications/ }).click();
      await openThread(page, /PR #482: Faster thread search/);
    },
  },
  {
    name: "html-email",
    capture: async (page) => {
      await openThread(page, /Your flight to Lisbon is confirmed/);
    },
  },
  {
    name: "reply",
    capture: async (page) => {
      await openThread(page, /Q4 launch plan — final review/);
      await page.getByTitle("Message content").contentFrame().locator("body").click();
      await page.keyboard.press("r");
      const reply = page.getByRole("dialog", { name: "Reply Message" });
      await expect(reply).toBeVisible();
      await reply.getByRole("textbox", { name: "Message Body" }).pressSequentially("Staging by region sounds right to me — see you Thursday.");
    },
  },
  {
    name: "command-palette",
    capture: async (page) => {
      await waitForMessage(page);
      await page.keyboard.press("ControlOrMeta+k");
      await expect(page.getByRole("dialog", { name: "Command Palette" })).toBeVisible();
    },
  },
  {
    name: "today-schedule",
    capture: async (page) => {
      await waitForMessage(page);
      await page.keyboard.press("T");
      await expect(page.getByText("Team standup").first()).toBeVisible();
    },
  },
  {
    name: "calendar-week",
    capture: async (page) => {
      await page.keyboard.press("2");
      await expect(page.getByRole("region", { name: "Calendar week" }).getByText("Q4 launch plan sign-off")).toBeVisible();
    },
  },
  {
    name: "tasks",
    capture: async (page) => {
      await page.keyboard.press("3");
      await expect(page.getByText("Sign off on the Q4 launch plan").first()).toBeVisible();
    },
  },
  {
    name: "contacts",
    capture: async (page) => {
      await page.getByRole("button", { name: "Contacts", exact: true }).click();
      await expect(page.getByRole("textbox", { name: "Name" })).toHaveValue("Priya Natarajan");
      await expect(page.getByRole("heading", { name: "Recent emails" })).toBeVisible();
    },
  },
];

for (const theme of ["light", "dark"] as const) {
  test.describe(`${theme} theme`, () => {
    test.use({ colorScheme: theme });

    for (const scene of scenes) {
      test(scene.name, async ({ page }) => {
        await page.clock.setFixedTime(frozenNow);
        await page.addInitScript((value) => {
          try {
            localStorage.setItem("threestrands.theme", value);
          } catch {
            // The emulated color scheme still selects the theme.
          }
        }, theme);
        await page.goto("/");
        await expect(page.getByRole("heading", { name: /\d+ conversations?/ })).toBeVisible();

        await scene.capture(page);

        // Park the pointer so no hover tooltip or row highlight leaks into the shot.
        const viewport = page.viewportSize()!;
        await page.mouse.move(viewport.width - 1, viewport.height - 1);
        await page.screenshot({
          path: path.join(outputDir, theme, `${scene.name}.png`),
          animations: "disabled",
          caret: "hide",
        });
      });
    }
  });
}
