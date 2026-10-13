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
  const search = files.getByRole("searchbox", { name: "Search Files" });
  await search.fill("5");
  await expect(search).toHaveValue("5");
  await expect(files).toBeVisible();
});


test("Files keeps secondary filters optional and shows explicit empty dates", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("region", { name: "Inbox" })).toBeVisible();
  await page.keyboard.press("5");
  const files=page.getByRole("region", { name: "Files", exact: true });
  await expect(files.getByRole("listitem").first()).toBeVisible();
  await expect(files.getByText("Download required", { exact: true })).toHaveCount(0);
  await expect(files.getByText("Availability not yet known", { exact: true })).toHaveCount(0);
  const filters=files.getByRole("group", { name: "File filters", exact: true });
  await expect(filters).toBeHidden();
  await files.getByRole("button", { name: "Filters", exact: true }).click();
  await expect(filters).toBeVisible();
  await expect(filters.getByLabel("From Date", { exact: true })).toHaveValue("");
  await expect(filters.getByLabel("Through Date", { exact: true })).toHaveValue("");
  await expect(filters.getByText("Any date", { exact: true })).toHaveCount(2);
  await filters.getByLabel("Sender", { exact: true }).fill("hello");
  await filters.getByLabel("Sender", { exact: true }).press("Escape");
  await expect(filters).toBeHidden();
  await files.getByRole("button", { name: "Remove Sender: hello filter", exact: true }).click();
  await expect(files.getByRole("group", { name: "Active file filters" })).toHaveCount(0);
  await expect(files.getByRole("complementary", { name: "File context" }).getByRole("button", { name: "Save as…", exact: true })).toBeVisible();
});
