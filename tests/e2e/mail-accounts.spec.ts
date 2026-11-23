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

test("opens Signature Defaults and Sending Addresses from an IMAP account menu with mouse or keyboard", async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 600 });
  await page.goto("/");
  await page.getByRole("button", { name: /^Settings \((⌘|Ctrl\+),\)$/ }).click();
  const settings = page.getByRole("dialog", { name: "Settings", exact: true });
  await settings.getByRole("button", { name: "Mail Accounts", exact: true }).click();

  // Switch the demo account to IMAP so the IMAP card renders with its menu.
  const actions = settings.getByRole("button", { name: "More actions for demo@example.com", exact: true });
  await actions.click();
  await page.getByRole("menuitem", { name: "Switch to Gmail via IMAP…", exact: true }).click();
  const switchDialog = page.getByRole("dialog", { name: "Switch to Gmail via IMAP", exact: true });
  await switchDialog.getByLabel("App Password", { exact: true }).fill("abcd efgh ijkl mnop");
  await switchDialog.getByRole("button", { name: "Test Connection", exact: true }).click();
  await switchDialog.getByRole("button", { name: "Switch to Gmail via IMAP", exact: true }).click();
  await expect(switchDialog).toHaveCount(0);

  await expect(settings.getByText("IMAP", { exact: true })).toBeVisible();
  await expect(settings.getByLabel("Reply-To (optional)")).toHaveCount(0);

  await actions.click();
  await expect(page.getByRole("menuitem")).toHaveText(["Signature Defaults…", "Sending Addresses…"]);
  await page.getByRole("menuitem", { name: "Signature Defaults…", exact: true }).click();
  const signaturesDialog = page.getByRole("dialog", { name: "Default Signatures", exact: true });
  await expect(signaturesDialog).toBeVisible();
  await expect(signaturesDialog.getByRole("combobox", { name: "Sending Address", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(signaturesDialog).toHaveCount(0);
  await expect(actions).toBeFocused();

  await actions.click();
  await page.getByRole("menuitem", { name: "Sending Addresses…", exact: true }).click();
  const sendingDialog = page.getByRole("dialog", { name: "Sending Addresses", exact: true });
  await expect(sendingDialog).toBeVisible();
  await expect(sendingDialog.getByText("demo@example.com", { exact: true })).toBeVisible();
  await sendingDialog.getByRole("button", { name: "Add address", exact: true }).click();
  await sendingDialog.getByLabel("Address 1", { exact: true }).fill("alias@example.com");
  await sendingDialog.getByRole("button", { name: "Save addresses", exact: true }).click();
  await expect(sendingDialog.getByRole("status")).toHaveText("Addresses saved.");
  await page.keyboard.press("Escape");
  await expect(sendingDialog).toHaveCount(0);
  await expect(actions).toBeFocused();

  await actions.press("ArrowDown");
  await page.keyboard.press("End");
  await expect(page.getByRole("menuitem", { name: "Sending Addresses…", exact: true })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(sendingDialog).toBeVisible();
  await sendingDialog.getByRole("button", { name: "Close", exact: true }).click();
  await expect(sendingDialog).toHaveCount(0);
  await expect(actions).toBeFocused();
});
