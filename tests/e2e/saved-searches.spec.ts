import { expect, test } from "@playwright/test";

test("saves the current search and restores its scope and Inbox Only after restart", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("listbox", { name: "Inbox" })).toBeVisible();
  await page.keyboard.press("/");
  const input = page.getByRole("textbox", { name: "Search Mail" });
  await input.fill("Welcome");
  await page.getByRole("button", { name: "Search only the Inbox" }).click();
  await page.getByRole("button", { name: "Save Search", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Save Search", exact: true });
  await dialog.getByRole("textbox", { name: "Name", exact: true }).fill("Welcome mail");
  await dialog.getByRole("combobox", { name: "Account scope" }).selectOption("demo@example.com");
  await dialog.getByRole("button", { name: "Save Search", exact: true }).click();
  await expect(dialog).not.toBeVisible();

  await page.reload();
  await expect(page.getByRole("listbox", { name: "Inbox" })).toBeVisible();
  await page.keyboard.press("/");
  await page.getByRole("button", { name: "Saved searches", exact: true }).click();
  await page.getByRole("button", { name: "Welcome mail · demo@example.com · Inbox Only", exact: true }).click();
  await expect(input).toHaveValue("Welcome");
  await expect(page.getByRole("button", { name: "Search only the Inbox" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("listbox", { name: "Inbox" }).getByRole("option")).toHaveCount(1);
  await expect(page.getByRole("listbox", { name: "Inbox" }).getByRole("option")).toContainText("Welcome to ThreeStrands");
  await page.getByRole("button", { name: "Saved searches", exact: true }).click();
  await page.getByRole("button", { name: "Delete saved search Welcome mail for demo@example.com" }).click();
  await expect(page.getByRole("button", { name: "Welcome mail · demo@example.com · Inbox Only", exact: true })).toHaveCount(0);
});

test("shows five deliberate recent searches and keeps Close near the input's right edge", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("listbox", { name: "Inbox" })).toBeVisible();
  await page.keyboard.press("/");
  const input = page.getByRole("textbox", { name: "Search Mail" });
  for (let index = 1; index <= 6; index += 1) {
    await input.fill(`query${index}`);
    await input.press("Enter");
  }
  await input.fill("unfinished");
  await page.getByRole("button", { name: "Saved searches", exact: true }).click();
  await page.keyboard.press("Escape");
  await input.fill("");
  await expect(page.getByRole("heading", { name: "Recent searches" })).toBeVisible();
  const recent = page.getByRole("listbox", { name: "Recent searches" });
  await expect(recent.getByRole("option")).toHaveText(["query6", "query5", "query4", "query3", "query2"]);
  const box = await page.locator(".search-box").boundingBox();
  const close = await page.getByRole("button", { name: "Close Search", exact: true }).boundingBox();
  expect(box).not.toBeNull();
  expect(close).not.toBeNull();
  expect(box!.x + box!.width - close!.x - close!.width).toBeLessThanOrEqual(6);
});
