import { expect, test } from "@playwright/test";

test("snoozes from the keyboard, reschedules, returns to Inbox, and undoes", async ({ page }) => {
  await page.goto("/");
  const selected = page.getByRole("listbox").getByRole("option", { selected: true });
  await expect(selected).toBeVisible();
  const subject = await selected.locator(".thread-subject").innerText();
  await selected.focus();
  await page.keyboard.press("h");
  const dialog = page.getByRole("dialog", { name: "Snooze Conversation", exact: true });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Tomorrow", exact: true }).click();
  await expect(dialog).not.toBeVisible();
  await expect(page.getByRole("listbox").getByRole("option").filter({ hasText: subject })).toHaveCount(0);
  await page.keyboard.press("g"); await page.keyboard.press("z");
  const row = page.getByRole("listbox", { name: "Snoozed" }).getByRole("option").filter({ hasText: subject });
  await expect(row).toBeVisible();
  await expect(row).toContainText("Snoozed until");
  await row.click();
  await page.getByRole("button", { name: /^Reschedule Snooze/ }).click();
  const reschedule = page.getByRole("dialog", { name: "Reschedule Snooze" });
  await reschedule.getByRole("button", { name: "Next week", exact: true }).click();
  await expect(reschedule).not.toBeVisible();
  await page.getByRole("button", { name: "Return to Inbox", exact: true }).click();
  await expect(row).toHaveCount(0);
  await page.keyboard.press("z");
  await expect(row).toBeVisible();
  await row.focus(); await page.keyboard.press("e");
  await expect(row).toHaveCount(0);
  await page.keyboard.press("z");
  await expect(row).toBeVisible();
  await page.keyboard.press("g"); await page.keyboard.press("i");
  await expect(page.getByRole("listbox", { name: "Inbox" }).getByRole("option").filter({ hasText: subject })).toHaveCount(0);
});

test("snoozes checked conversations together and cancels without changing mail", async ({ page }) => {
  await page.goto("/");
  const rows = page.getByRole("listbox").getByRole("option");
  await expect(rows.first()).toBeVisible();
  const subjects = await rows.locator(".thread-subject").allInnerTexts();
  await rows.first().focus(); await page.keyboard.press("x");
  await page.keyboard.press("j"); await page.keyboard.press("x");
  const toolbar = page.getByRole("toolbar", { name: "Batch actions" });
  await toolbar.getByRole("button", { name: "Snooze", exact: true }).click();
  let dialog = page.getByRole("dialog", { name: "Snooze 2 Conversations" });
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(await rows.locator(".thread-subject").allInnerTexts()).toEqual(subjects);
  await toolbar.getByRole("button", { name: "Snooze", exact: true }).click();
  dialog = page.getByRole("dialog", { name: "Snooze 2 Conversations" });
  await dialog.getByRole("button", { name: "Tomorrow", exact: true }).click();
  await page.keyboard.press("g"); await page.keyboard.press("z");
  await expect(page.getByRole("listbox", { name: "Snoozed" }).getByRole("option")).toHaveCount(2);
});


test("b creates an event from Mail and Calendar without snoozing mail", async ({ page }) => {
  await page.goto("/");
  const rows = page.getByRole("listbox").getByRole("option");
  await expect(rows.first()).toBeVisible();
  await rows.first().focus();
  await page.keyboard.press("b");
  const dialog = page.getByRole("dialog", { name: "New Event" });
  await expect(dialog).toBeVisible();
  await expect(page.getByRole("dialog", { name: "Snooze Conversation" })).toHaveCount(0);
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.keyboard.press("b");
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.getByRole("button", { name: "Inbox (1)" }).click();
  await expect(page.getByRole("listbox").getByRole("option")).not.toHaveCount(0);
  await page.keyboard.press("g"); await page.keyboard.press("z");
  await expect(page.getByRole("listbox", { name: "Snoozed" }).getByRole("option")).toHaveCount(0);
});
