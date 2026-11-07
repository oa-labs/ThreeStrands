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
