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

/** Browser-only demo key: no credentials or provider requests are involved. */
async function configureDemoAi(page: Page, features: string[]) {
  await page.getByRole("button", { name: /^Settings/ }).click();
  const settings = page.getByRole("dialog", { name: "Settings", exact: true });
  await settings.getByRole("button", { name: "AI Provider", exact: true }).click();
  await settings.getByRole("combobox", { name: "Provider", exact: true }).selectOption("openai");
  await settings.getByLabel("API Key", { exact: true }).fill("fictional-showcase-key");
  await settings.getByRole("button", { name: "Save Key", exact: true }).click();
  await expect(settings.getByText("API key configured", { exact: true })).toBeVisible();
  for (const feature of features) await settings.getByRole("checkbox", { name: feature, exact: true }).check();
  await page.keyboard.press("Escape");
  await expect(settings).toBeHidden();
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
    name: "calendar-create",
    capture: async (page) => {
      await page.keyboard.press("2");
      await page.getByRole("button", { name: "New Event", exact: true }).click();
      const event = page.getByRole("dialog", { name: "New event", exact: true });
      await event.getByLabel("Title", { exact: true }).fill("Q4 launch rollout review");
      await event.getByLabel("Invite people", { exact: true }).fill("priya@harborlight.example");
      await event.getByLabel("Description", { exact: true }).fill("Confirm North America first, then EU and APAC 48 hours later.");
      await expect(event.getByRole("button", { name: "Create event", exact: true })).toBeEnabled();
      await expect(event.getByRole("combobox", { name: "Calendar", exact: true })).not.toHaveValue("");
    },
  },
  {
    name: "thread-assist",
    capture: async (page) => {
      await configureDemoAi(page, ["Thread Summaries", "Suggestions"]);
      await openThread(page, /Contract renewal — Brightwater Co-op/);
      const brief = page.getByRole("region", { name: "Brief", exact: true });
      await brief.getByRole("button", { name: "Get Brief", exact: true }).click();
      await expect(brief.getByText("Send Marcus pricing for 35 seats", { exact: true })).toBeVisible();
      await brief.getByText("From the email", { exact: true }).click();
      await expect(brief.locator("blockquote")).toBeVisible();
      await brief.getByRole("button", { name: "Review & Add Task", exact: true }).scrollIntoViewIfNeeded();
      await expect(brief.locator("blockquote")).toBeInViewport();
      await expect(brief.locator("blockquote")).toContainText("Could you send over updated pricing for 35 seats");
      await expect(brief.getByText("Brightwater is renewing its annual plan and needs pricing for 35 seats.", { exact: true })).toBeVisible();
    },
  },
  {
    name: "thread-chat",
    capture: async (page) => {
      await configureDemoAi(page, ["Thread Chat"]);
      await openThread(page, /Q4 launch plan — final review/);
      await page.keyboard.press("q");
      const chat = page.getByRole("region", { name: "Ask about this conversation", exact: true });
      const question = chat.getByRole("textbox", { name: "Ask about this conversation", exact: true });
      await question.fill("@Q4");
      await chat.getByRole("option", { name: /Q4-launch-plan-v7.pdf/ }).click();
      await question.press("End");
      await question.pressSequentially(" What changed in onboarding and how will we roll it out?");
      await chat.getByRole("checkbox", { name: "Search all mail", exact: true }).check();
      await chat.getByRole("button", { name: "Ask", exact: true }).click();
      await expect(chat.getByRole("log")).toContainText("EU and APAC following 48 hours later");
      // Submitting can return the dock to read mode; reopen it to show shared files.
      const prompt = chat.getByRole("button", { name: "Ask about this conversation…", exact: true });
      if (await prompt.isVisible()) await prompt.click();
      await chat.getByRole("log").evaluate((element) => { element.scrollTop = 0; });
      await chat.getByRole("navigation", { name: "Sources" }).scrollIntoViewIfNeeded();
      await expect(chat.getByRole("navigation", { name: "Sources" })).toBeInViewport();
      await expect(chat.getByRole("list", { name: "Attachments shared with AI" })).toBeInViewport();
      await expect(chat.getByRole("log")).toContainText("Shared Q4-launch-plan-v7.pdf");
    },
  },
  {
    name: "tasks",
    capture: async (page) => {
      await page.keyboard.press("3");
      await expect(page.getByText("Sign off on the Q4 launch plan").first()).toBeVisible();
      await expect(page.getByText("Prepare the team retrospective agenda", { exact: true })).toBeVisible();
    },
  },
  {
    name: "goals",
    capture: async (page) => {
      await page.keyboard.press("3");
      const goals = page.getByRole("complementary", { name: "Goals" });
      await goals.getByRole("button", { name: /^Ship onboarding v3 with the Q4 launch/ }).click();
      await expect(page.getByText("Sign off on the Q4 launch plan").first()).toBeVisible();
      await expect(page.getByText("Send Marcus pricing for 35 seats", { exact: true })).toBeHidden();
    },
  },
  {
    name: "contacts",
    capture: async (page) => {
      await page.keyboard.press("4");
      await expect(page.getByRole("textbox", { name: "Name" })).toHaveValue("Priya Natarajan");
      await expect(page.getByRole("heading", { name: "Recent emails" })).toBeVisible();
      await expect(page.getByText("priya.natarajan@harborlight.example", { exact: true })).toBeVisible();
    },
  },
];

for (const theme of ["light", "dark"] as const) {
  test.describe(`${theme} theme`, () => {
    test.use({ colorScheme: theme });

    for (const scene of scenes) {
      test(scene.name, async ({ page, baseURL }) => {
        const foreignRequests: string[] = [];
        const origin = new URL(baseURL!).origin;
        await page.route("**/*", async (route) => {
          const url = new URL(route.request().url());
          if (url.protocol.startsWith("http") && url.origin !== origin) {
            foreignRequests.push(url.href);
            await route.abort();
          } else await route.continue();
        });
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

        // Fonts and decoded images must settle before the reproducible capture.
        for (const frame of page.frames()) {
          await frame.evaluate(async () => {
            await document.fonts.ready;
            await Promise.all([...document.images].filter((image) => image.src).map((image) => image.decode()));
          });
        }
        expect(foreignRequests).toEqual([]);

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
