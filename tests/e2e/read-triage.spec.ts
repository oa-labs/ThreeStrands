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
