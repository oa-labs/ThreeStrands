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

test("a [[link]] picks a note, opens it, shows as a backlink, and follows a rename", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.keyboard.press("4");
  const notes = page.getByRole("region", { name: "Notes" });
  await expect(notes.getByRole("button", { name: /^Today/ })).toHaveAttribute("aria-current", "true");
  const create = async (title: string) => {
    await page.keyboard.press("Shift+N");
    await expect(page.getByRole("dialog", { name: "New Note" }).getByRole("textbox", { name: "Title" })).toBeFocused();
    await page.keyboard.type(title);
    await page.keyboard.press("Enter");
    await expect(notes.getByRole("textbox", { name: "Note text" })).toBeFocused();
  };
  await create("Garden plan");
  await page.keyboard.press("Escape");
  await create("Weekend");
  await page.keyboard.type("Work on [[Garden");
  await page.getByRole("option", { name: /Garden plan/ }).click();
  const body = notes.getByRole("textbox", { name: "Note text" });
  const link = body.locator(".md-wikilink-view-preview", { hasText: "Garden plan" });
  await expect(link).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(notes.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();

  await link.click();
  await expect(notes.getByRole("textbox", { name: "Title" })).toHaveValue("Garden plan");
  const backlinks = page.getByRole("complementary", { name: "Note context" }).getByRole("region", { name: /Backlinks/ });
  await expect(backlinks.getByRole("button", { name: /Weekend/ })).toContainText("Work on [[Garden plan]]");

  const title = notes.getByRole("textbox", { name: "Title" });
  await title.fill("Vegetable garden");
  await title.press("Enter");
  await expect(notes.getByRole("status")).toContainText("Updated links in 1 note.");
  await backlinks.getByRole("button", { name: /Weekend/ }).click();
  await expect(body.locator(".md-wikilink-view-preview", { hasText: "Vegetable garden" })).toBeVisible();
});

test("the editor's menus stay hidden until they open, then appear at the cursor", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.keyboard.press("4");
  const notes = page.getByRole("region", { name: "Notes" });
  const body = notes.getByRole("textbox", { name: "Note text" });
  await expect(body).toBeVisible();
  const visibleMenus = () => page.evaluate(() => [...document.querySelectorAll("[popover]")].filter((menu) => {
    const box = menu.getBoundingClientRect();
    return getComputedStyle(menu).display !== "none" && box.width > 0 && box.height > 0;
  }).map((menu) => menu.tagName.toLowerCase()));
  expect(await visibleMenus()).toEqual([]);

  await body.click();
  await page.keyboard.type("/");
  const slash = page.getByTestId("slash-menu");
  await expect(slash).toBeVisible();
  const caret = await page.evaluate(() => window.getSelection()!.getRangeAt(0).getBoundingClientRect().bottom);
  const menu = (await slash.boundingBox())!;
  expect(menu.y).toBeGreaterThanOrEqual(caret - 1);
  expect(menu.y - caret).toBeLessThan(80);
  await page.keyboard.press("Escape");
  await expect(slash).toBeHidden();
});

test("list items and paragraphs sit close together, at the reading size", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.keyboard.press("4");
  const body = page.getByRole("region", { name: "Notes" }).getByRole("textbox", { name: "Note text" });
  await body.click();
  await page.keyboard.type("## The Basics\n- one\ntwo\nthree\n\nA paragraph\n\nAnother paragraph");
  const layout = await body.evaluate((root) => {
    const tops = (selector: string) => [...root.querySelectorAll(`:scope > ${selector}`)].map((block) => block.getBoundingClientRect().top);
    const style = getComputedStyle(root);
    return { items: tops(".prosemirror-flat-list"), paragraphs: tops("p"), lineHeight: parseFloat(style.lineHeight), fontSize: style.fontSize };
  });
  expect(layout.items).toHaveLength(3);
  // Each line plus a small gap: no paragraph margins collapsing between items.
  for (const tops of [layout.items, layout.paragraphs]) {
    for (let index = 1; index < tops.length; index++) {
      const pitch = tops[index]! - tops[index - 1]!;
      expect(pitch).toBeGreaterThan(layout.lineHeight);
      expect(pitch).toBeLessThanOrEqual(layout.lineHeight + 10);
    }
  }
  expect(layout.fontSize).toBe("15px");
});
