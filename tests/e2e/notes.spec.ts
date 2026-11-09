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
  await expect(body).toHaveText("Ship 4 things");
  await expect(notes).toBeVisible();

  await page.keyboard.press("Escape");
  const row = notes.getByRole("button", { name: /Launch plan/ });
  await expect(row).toBeFocused();
  await expect(notes.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();
  await page.keyboard.press("k");
  await expect(notes.getByRole("button", { name: /^Today/ })).toBeFocused();
  await page.keyboard.press("j");
  await expect(body).toHaveText("Ship 4 things");
});

test("the note editor formats Markdown as you type and keeps raw HTML as text", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.keyboard.press("4");
  const notes = page.getByRole("region", { name: "Notes" });
  await expect(notes.getByRole("button", { name: /^Today/ })).toHaveAttribute("aria-current", "true");
  await page.keyboard.press("Shift+N");
  await expect(page.getByRole("dialog", { name: "New Note" }).getByRole("textbox", { name: "Title" })).toBeFocused();
  await page.keyboard.type("Formatting");
  await page.keyboard.press("Enter");
  const body = notes.getByRole("textbox", { name: "Note text" });
  await expect(body).toBeFocused();
  await page.keyboard.type("Some **bold** words");
  await page.keyboard.press("Enter");
  await page.keyboard.type("<img src=x onerror=alert(1)>");
  await expect(body.locator("strong, b").filter({ hasText: "bold" })).toHaveCount(1);
  await expect(body).toContainText("<img src=x onerror=alert(1)>");
  await expect(body.locator("img")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(notes.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();
});

test("# moves a note to Trash, and Trash restores it", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.keyboard.press("4");
  const notes = page.getByRole("region", { name: "Notes" });
  await expect(notes.getByRole("button", { name: /^Today/ })).toHaveAttribute("aria-current", "true");
  await page.keyboard.press("Shift+N");
  await expect(page.getByRole("dialog", { name: "New Note" }).getByRole("textbox", { name: "Title" })).toBeFocused();
  await page.keyboard.type("Short lived");
  await page.keyboard.press("Enter");
  await expect(notes.getByRole("textbox", { name: "Note text" })).toBeFocused();
  await page.keyboard.type("Remember this");
  await page.keyboard.press("Escape");
  await expect(notes.getByRole("button", { name: /Short lived/ })).toBeFocused();

  await page.keyboard.press("#");
  await expect(notes.getByRole("list", { name: "Notes" }).getByRole("button", { name: /Short lived/ })).toHaveCount(0);
  await notes.getByRole("button", { name: /^Trash/ }).click();
  await notes.getByRole("list", { name: "Trash" }).getByRole("button", { name: /Short lived/ }).click();
  const trashed = notes.getByRole("region", { name: "Note in Trash" });
  await expect(notes.getByRole("textbox", { name: "Note text" })).toHaveAttribute("contenteditable", "false");
  await trashed.getByRole("button", { name: "Restore" }).click();
  await expect(trashed).toBeHidden();
  await expect(notes.getByRole("textbox", { name: "Note text" })).toHaveText("Remember this");
  await notes.getByRole("button", { name: "Back to Notes" }).click();
  await expect(notes.getByRole("list", { name: "Notes" }).getByRole("button", { name: /Short lived/ })).toBeVisible();
});
