import { expect, test } from "@playwright/test";

test("4 opens Notes, Enter writes in a new note, and Escape returns to the list", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.keyboard.press("4");
  const notes = page.getByRole("region", { name: "Notes" });
  await expect(notes.getByRole("button", { name: /^Today/ })).toHaveAttribute("aria-current", "true");

  await page.keyboard.press("Shift+N");
  const dialog = page.getByRole("dialog", { name: "New Note" });
  await expect(dialog.getByRole("textbox", { name: "Title" })).toBeFocused();
  await page.keyboard.type("Launch plan");
  await page.keyboard.press("Enter");
  await expect(dialog).toBeHidden();
  const body = notes.getByRole("textbox", { name: "Note text" });
  await expect(body).toBeFocused();
  // Shortcut keys type into the note.
  await page.keyboard.type("Ship 4 things");
  await expect(body).toHaveValue("Ship 4 things");
  await expect(notes).toBeVisible();

  await page.keyboard.press("Escape");
  const row = notes.getByRole("button", { name: /Launch plan/ });
  await expect(row).toBeFocused();
  await expect(notes.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();
  await page.keyboard.press("k");
  await expect(notes.getByRole("button", { name: /^Today/ })).toBeFocused();
  await page.keyboard.press("j");
  await expect(body).toHaveValue("Ship 4 things");
});
