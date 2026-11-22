import { expect, test } from "@playwright/test";

test("opens the Gmail IMAP switch from the account menu with mouse or keyboard", async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 480 });
  await page.goto("/");
  await page.getByRole("button", { name: /^Settings \((⌘|Ctrl\+),\)$/ }).click();
  const settings = page.getByRole("dialog", { name: "Settings", exact: true });
  await settings.getByRole("button", { name: "Mail Accounts", exact: true }).click();
  const actions = settings.getByRole("button", { name: "More actions for demo@example.com", exact: true });
  const switchItem = page.getByRole("menuitem", { name: "Switch to Gmail via IMAP…", exact: true });
  const dialog = page.getByRole("dialog", { name: "Switch to Gmail via IMAP", exact: true });
  const password = page.getByLabel("App Password", { exact: true });

  await actions.click();
  await switchItem.click();
  await expect(password).toBeInViewport();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByLabel("App Password")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await expect(actions).toBeFocused();

  await actions.click();
  await expect(switchItem).toBeVisible();
  await actions.click();
  await expect(switchItem).toHaveCount(0);

  await actions.press("ArrowDown");
  await page.keyboard.press("End");
  await expect(switchItem).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(dialog.getByLabel("App Password")).toBeFocused();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(actions).toBeFocused();
});
