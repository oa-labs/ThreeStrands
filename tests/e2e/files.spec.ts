import { expect, test } from "@playwright/test";

test("5 opens Files, stars an untouched attachment and returns from its source email", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("region", { name: "Inbox" })).toBeVisible();
  await page.keyboard.press("5");
  const files = page.getByRole("region", { name: "Files", exact: true });
  await expect(files).toBeVisible();
  const attachments = files.getByRole("list", { name: "Mail attachments" });
  const row = attachments.getByRole("listitem").filter({ hasText: "threestrands-shortcuts.txt" });
  await expect(row).toBeVisible();
  await row.getByRole("button", { name: "Star threestrands-shortcuts.txt" }).click();
  await expect(row.getByRole("button", { name: "Unstar threestrands-shortcuts.txt" })).toBeVisible();
  await files.getByRole("button", { name: "Starred", exact: true }).click();
  await expect(row).toBeVisible();
  await row.locator(".file-list-select").click();
  await files.getByRole("complementary", { name: "File context" }).getByRole("button", { name: /Welcome to ThreeStrands/ }).click();
  await expect(page.getByRole("region", { name: "Conversation", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(files).toBeVisible();
  await expect(files.getByRole("button", { name: "Starred", exact: true })).toHaveAttribute("aria-pressed", "true");
  const search = files.getByRole("searchbox", { name: "Search files" });
  await search.fill("5");
  await expect(search).toHaveValue("5");
  await expect(files).toBeVisible();
});
