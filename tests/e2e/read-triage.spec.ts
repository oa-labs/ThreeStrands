import { expect, test } from "@playwright/test";

test("processes the inbox from the keyboard", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to Dispatch" })).toBeVisible();

  await page.keyboard.press("s");
  await expect(page.getByRole("button", { name: "Unstar (s)" })).toBeVisible();

  await page.keyboard.press("j");
  await expect(page.getByRole("heading", { name: "Phase 1: read and triage" })).toBeVisible();

  await page.keyboard.press("e");
  await expect(page.getByRole("status")).toContainText("Conversation archived");
  await expect(page.getByRole("heading", { name: "2 conversations" })).toBeVisible();
  await expect(page.getByRole("option", { selected: true })).toContainText("Your inbox stays local");
  await expect(page.getByRole("heading", { name: "Your inbox stays local" })).toBeVisible();
});

test("searches and opens the command palette", async ({ page }) => {
  await page.goto("/");
  await page.keyboard.press("/");
  await page.getByRole("textbox", { name: "Search mail" }).fill("SQLite");
  await expect(page.getByRole("option")).toHaveCount(1);
  await expect(page.getByRole("heading", { name: "Your inbox stays local" })).toBeVisible();

  await page.keyboard.press("ControlOrMeta+k");
  await expect(page.getByRole("dialog", { name: "Command palette" })).toBeVisible();
});

test("switches themes and remembers the choice after reload", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/");
  await page.getByRole("button", { name: "Switch to light mode" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator(".reader")).toHaveCSS("background-color", "rgb(255, 255, 255)");
  await page.reload();
  await expect(page.getByRole("button", { name: "Switch to dark mode" })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.getByRole("button", { name: "Switch to dark mode" }).click();
  await expect(page.locator(".reader")).toHaveCSS("background-color", "rgb(18, 18, 20)");
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
});

test("resizes the inbox with pointer and keyboard and restores the preferred width", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  const divider = page.getByRole("separator", { name: "Resize inbox" });
  await expect(divider).toHaveAttribute("aria-valuenow", "400");
  const bounds = (await divider.boundingBox())!;
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + 150);
  await page.mouse.down();
  await page.mouse.move(bounds.x + bounds.width / 2 + 120, bounds.y + 150);
  await page.mouse.up();
  await expect(page.locator(".thread-column")).toHaveCSS("width", "520px");
  await page.reload();
  await expect(divider).toHaveAttribute("aria-valuenow", "520");
  await divider.focus();
  await page.keyboard.press("ArrowLeft");
  await expect(divider).toHaveAttribute("aria-valuenow", "510");
  await page.setViewportSize({ width: 900, height: 800 });
  await expect(divider).toHaveAttribute("aria-valuenow", "422");
  await page.setViewportSize({ width: 1280, height: 800 });
  await expect(divider).toHaveAttribute("aria-valuenow", "510");
  await page.keyboard.press("Home");
  await expect(divider).toHaveAttribute("aria-valuenow", "280");
  await divider.dblclick();
  await expect(divider).toHaveAttribute("aria-valuenow", "400");
});

test("keeps the theme toggle on screen with a long email", async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 600 });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/");
  await expect(page.getByTestId("message-body")).toBeVisible();
  await page.getByTestId("message-body").evaluate((body) => {
    body.innerHTML = '<p>A long email paragraph.</p>'.repeat(150);
  });
  const toggle = page.getByRole("button", { name: "Switch to light mode" });
  await expect(toggle).toBeInViewport();
  await toggle.click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator(".message-stack")).toHaveJSProperty("scrollTop", 0);
});
