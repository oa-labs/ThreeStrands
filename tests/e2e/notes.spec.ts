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
  // The backlink shows the linking line as text, not its Markdown.
  await expect(backlinks.getByRole("button", { name: /Weekend/ })).toContainText("Work on Garden plan");

  const title = notes.getByRole("textbox", { name: "Title" });
  await title.fill("Vegetable garden");
  await title.press("Enter");
  await expect(notes.getByRole("status").filter({ hasText: "Updated links" })).toHaveText("Updated links in 1 note.");
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

test("conversation capture stays in Mail, opens today's reference, and returns to the same message", async ({ page }) => {
  await page.goto("/");
  const selected = page.getByRole("listbox").getByRole("option", { selected: true });
  await expect(selected).toBeVisible();
  const subject = await selected.locator(".thread-subject").innerText();
  const reader = page.getByRole("region", { name: "Conversation" });
  await expect(reader.locator(".message-active")).toHaveCount(1);
  const active = await reader.locator(".message-active").getAttribute("data-message-id");
  const expanded = await reader.locator(".message-card-expanded").evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-message-id")));
  await reader.getByRole("button", { name: "Conversation actions" }).click();
  await page.getByRole("menuitem", { name: "Add to Today's Note" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Added to today's note" })).toBeVisible();
  await expect(reader.locator(".message-active")).toHaveAttribute("data-message-id", active!);
  await page.getByRole("button", { name: "Open Note", exact: true }).click();
  const notes = page.getByRole("region", { name: "Notes", exact: true });
  await expect(notes.getByRole("textbox", { name: "Note text" })).toContainText(subject);
  await notes.getByRole("button", { name: "Back to Mail" }).click();
  await expect(reader.locator(".message-active")).toHaveAttribute("data-message-id", active!);
  expect(await reader.locator(".message-card-expanded").evaluateAll((nodes) => nodes.map((node) => node.getAttribute("data-message-id")))).toEqual(expanded);
  await reader.getByRole("button", { name: "Conversation actions" }).click();
  await page.getByRole("menuitem", { name: "Add to Today's Note" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Already in today's note" })).toBeVisible();
  await page.keyboard.press("z");
  await expect(page.getByRole("complementary", { name: "Conversation context" }).getByRole("region", { name: "Notes", exact: true })).toHaveCount(0);
});

test("round note tasks reach Tasks while square checklists stay in their note", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.keyboard.press("4");
  const notes = page.getByRole("region", { name: "Notes", exact: true });
  await notes.getByRole("button", { name: /^Today/ }).click();
  const body = notes.getByRole("textbox", { name: "Note text" });
  await body.click();
  await page.keyboard.type("- [ ] Shopping only here");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Enter");
  await page.keyboard.type("- ( ) Send the proposal");
  await page.keyboard.press("Escape");
  await expect(notes.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();
  await expect(body.getByRole("checkbox", { name: "Checklist: Shopping only here" })).not.toBeChecked();
  await expect(body.getByRole("checkbox", { name: "Task: Send the proposal" })).not.toBeChecked();
  await page.keyboard.press("3");
  const tasks = page.getByRole("region", { name: "Tasks", exact: true });
  await tasks.getByRole("button", { name: "List", exact: true }).click();
  await expect(tasks.locator("strong").filter({ hasText: "Send the proposal" })).toBeVisible();
  await expect(tasks.locator("strong").filter({ hasText: "Shopping only here" })).toHaveCount(0);
  await page.getByRole("radio", { name: /^demo@example.com/ }).click();
  await expect(tasks).toBeVisible();
  await expect(tasks.locator("strong").filter({ hasText: "Send the proposal" })).toHaveCount(0);
  await page.getByRole("radio", { name: /All [Aa]ccounts/ }).click();
  await expect(tasks.locator("strong").filter({ hasText: "Send the proposal" })).toBeVisible();
  await tasks.getByRole("button", { name: "Complete Send the proposal" }).click();
  await page.keyboard.press("4");
  await expect(body.getByRole("checkbox", { name: "Task: Send the proposal" })).toBeChecked();
  await expect(body.getByRole("checkbox", { name: "Checklist: Shopping only here" })).not.toBeChecked();
});

test("adding a task after a saved task preserves the first task's identity", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.keyboard.press("4");
  const notes = page.getByRole("region", { name: "Notes", exact: true });
  const body = notes.getByRole("textbox", { name: "Note text" });
  await body.click();
  await page.keyboard.type("- ( ) First task");
  await page.keyboard.press("Escape");
  await expect(notes.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();
  await expect(body.locator("[data-html-comment]")).toHaveCount(1);
  const originalId = await body.locator("[data-html-comment]").getAttribute("data-html-comment");
  await body.locator("p").filter({ hasText: "First task" }).click();
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  await page.keyboard.type("Second task");
  await page.keyboard.press("Escape");
  await expect(notes.getByRole("status").filter({ hasText: "Saved" })).toBeVisible();
  const first = body.locator('[data-list-kind="task"]').filter({ has: page.locator("p", { hasText: "First task" }) });
  await expect(first.locator("[data-html-comment]")).toHaveAttribute("data-html-comment", originalId!);
  await expect(body.getByRole("checkbox", { name: "Task: Second task" })).not.toBeChecked();
});

test("a note's task link selects the exact task and its backlink returns to the note", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.evaluate(async () => {
    const clientPath = "/src/data/client.ts", referencePath = "/src/noteEntities.ts";
    const [{ mailClient }, { markdownEntityLink }] = await Promise.all([import(clientPath), import(referencePath)]);
    await mailClient.createTask({ accountId: "demo@example.com", threadId: null, subjectSnapshot: null, title: "First unrelated task", kind: "action" });
    const task = await mailClient.createTask({ accountId: "demo@example.com", threadId: null, subjectSnapshot: null, title: "Linked target task", kind: "action" });
    await mailClient.createNote("Task reference source", markdownEntityLink("Linked target task", { version: 1, kind: "task", id: task.id }));
  });
  await page.keyboard.press("4");
  const notes = page.getByRole("region", { name: "Notes", exact: true });
  await notes.getByRole("list", { name: "Notes" }).getByRole("button", { name: /Task reference source/ }).click();
  await page.getByRole("complementary", { name: "Note context" }).getByRole("region", { name: "Links", exact: true }).getByRole("button", { name: "Linked target task" }).click();
  const tasks = page.getByRole("region", { name: "Tasks", exact: true });
  await expect(tasks.locator(".task-card.selected strong")).toHaveText("Linked target task");
  const context = page.getByRole("complementary", { name: "Task context" });
  await context.getByRole("region", { name: "Notes", exact: true }).getByRole("button", { name: /Task reference source/ }).click();
  await expect(notes.getByRole("textbox", { name: "Title" })).toHaveValue("Task reference source");
});
