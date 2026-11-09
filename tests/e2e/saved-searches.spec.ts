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

  await page.getByRole("radio", { name: /^demo@example.com/ }).click();
  await page.getByRole("tab", { name: "Welcome mail", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Welcome mail", exact: true })).toHaveAttribute("aria-selected", "true");
  await page.reload();
  await expect(page.getByRole("listbox", { name: "Welcome mail" })).toBeVisible();
  await expect(input).toHaveValue("Welcome");
  await expect(page.getByRole("button", { name: "Search only the Inbox" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("listbox", { name: "Welcome mail" }).getByRole("option")).toHaveCount(1);
  await expect(page.getByRole("listbox", { name: "Welcome mail" }).getByRole("option")).toContainText("Welcome to ThreeStrands");
  await page.getByRole("button", { name: "Saved searches", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  const card = settings.getByRole("listitem").filter({ has: page.getByRole("textbox", { name: "Name for Welcome mail" }) });
  await card.getByRole("button", { name: "Delete", exact: true }).click();
  await settings.getByRole("button", { name: "Delete Split Inbox", exact: true }).click();
  await expect(settings.getByRole("textbox", { name: "Name for Welcome mail" })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("tab", { name: "Welcome mail", exact: true })).toHaveCount(0);

});

test("shows five deliberate recent searches and keeps Close near the input's right edge", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("listbox", { name: "Inbox" })).toBeVisible();
  await page.keyboard.press("/");
  const input = page.getByRole("textbox", { name: "Search Mail" });
  for (let index = 1; index <= 6; index += 1) {
    await input.fill(`query${index}`);
    await input.press("Enter");
  }
  await input.fill("unfinished");
  await page.getByRole("button", { name: "Saved searches", exact: true }).click();
  await page.keyboard.press("Escape");
  await input.fill("");
  await expect(page.getByRole("heading", { name: "Recent searches" })).toBeVisible();
  const recent = page.getByRole("listbox", { name: "Recent searches" });
  await expect(recent.getByRole("option")).toHaveText(["query6", "query5", "query4", "query3", "query2"]);
  const box = await page.locator(".search-box").boundingBox();
  const close = await page.getByRole("button", { name: "Close Search", exact: true }).boundingBox();
  expect(box).not.toBeNull();
  expect(close).not.toBeNull();
  expect(box!.x + box!.width - close!.x - close!.width).toBeLessThanOrEqual(6);
});

test("adopts an existing All accounts search as a tab and edits it in Split Inboxes", async ({ page }) => {
  await page.addInitScript(() => {
    if (!localStorage.getItem("threestrands.savedSearches")) localStorage.setItem("threestrands.savedSearches", JSON.stringify({ version: 1, searches: [
      { id: "legacy-shared", name: "Old view", query: "Welcome", accountId: null, inboxOnly: false, messageFilters: [] },
    ] }));
  });
  await page.goto("/");
  const tab = page.getByRole("tab", { name: "Old view", exact: true });
  await tab.click();
  await expect(page.getByRole("listbox", { name: "Old view" }).getByRole("option")).toHaveCount(1);
  await page.getByRole("button", { name: "Saved searches", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Edit Old view", exact: true }).click();
  const edit = page.getByRole("dialog", { name: "Edit Saved Search" });
  await edit.getByRole("textbox", { name: "Name", exact: true }).fill("Local mail");
  await edit.getByRole("textbox", { name: "Search", exact: true }).fill("local");
  await edit.getByRole("button", { name: "Save Search", exact: true }).click();
  await expect(edit).not.toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("tab", { name: "Local mail", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("textbox", { name: "Search Mail" })).toHaveValue("local");
  // Saving another view must not reset a temporary query in the active one.
  await page.getByRole("textbox", { name: "Search Mail" }).fill("Welcome");
  await page.getByRole("button", { name: "Save Search", exact: true }).click();
  const copy = page.getByRole("dialog", { name: "Save Search", exact: true });
  await copy.getByRole("textbox", { name: "Name", exact: true }).fill("Copy view");
  await copy.getByRole("button", { name: "Save Search", exact: true }).click();
  await expect(copy).not.toBeVisible();
  await expect(page.getByRole("textbox", { name: "Search Mail" })).toHaveValue("Welcome");
  await page.getByRole("button", { name: "Saved searches", exact: true }).click();
  await settings.getByRole("button", { name: "Move Copy view up", exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("tab").filter({ hasText: /Copy view|Local mail/ })).toHaveText(["Copy view", "Local mail"]);
  await page.getByRole("tab", { name: "Local mail", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Search Mail" })).toHaveValue("local");
  await page.getByRole("tab", { name: /^Main/ }).click();
  await expect(page.getByRole("textbox", { name: "Search Mail" })).toHaveValue("");
  await expect(page.getByRole("listbox", { name: "Inbox" }).getByRole("option")).toHaveCount(3);
  // Even with one account, both scopes remain reachable.
  await page.getByRole("radio", { name: /^demo@example.com/ }).click();
  await expect(page.getByRole("tab", { name: "Local mail", exact: true })).toHaveCount(0);
  await page.getByRole("radio", { name: /^All accounts/i }).click();
  await page.getByRole("tab", { name: "Local mail", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Search Mail" })).toHaveValue("local");
  await page.reload();
  await expect(page.getByRole("tab", { name: "Local mail", exact: true })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("tab").filter({ hasText: /Copy view|Local mail/ })).toHaveText(["Copy view", "Local mail"]);
});


test("keeps search controls compact and saved-search actions outside the input", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("listbox", { name: "Inbox" })).toBeVisible();
  await page.keyboard.press("/");
  const input = page.getByRole("textbox", { name: "Search Mail" });
  await input.fill("from:someone@example.com has:attachment quarterly report");
  const inbox = page.getByRole("button", { name: "Search only the Inbox" });
  const measure = async () => {
    const box = (await page.locator(".search-box").boundingBox())!;
    const text = (await input.boundingBox())!;
    const toggle = (await inbox.boundingBox())!;
    const close = (await page.getByRole("button", { name: "Close Search", exact: true }).boundingBox())!;
    const save = (await page.getByRole("button", { name: "Save Search", exact: true }).boundingBox())!;
    const manage = (await page.getByRole("button", { name: "Saved searches", exact: true }).boundingBox())!;
    expect(box.width - text.width).toBeLessThanOrEqual(110);
    expect(close.x - toggle.x - toggle.width).toBeLessThanOrEqual(2);
    expect(save.x).toBeGreaterThan(box.x + box.width);
    expect(manage.x - save.x - save.width).toBeLessThanOrEqual(2);
    return text.width;
  };
  const width = await measure();
  await inbox.click();
  await expect(inbox).toHaveAttribute("aria-pressed", "true");
  expect(await measure()).toBe(width);
});
