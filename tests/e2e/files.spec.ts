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

test("reader attachment chips share a right edge per column and show each target's tooltip", async ({ page }) => {
  await page.goto("/");
  const open = page.getByRole("button", { name: "Open threestrands-shortcuts.txt" });
  await expect(open).toBeVisible();
  // The demo message has one attachment; clone its chip with shorter and
  // longer names so the grid lays out a column of differently sized chips.
  await page.evaluate(() => {
    const chip = document.querySelector(".message-attachment")!;
    for (const name of ["a", "a-much-longer-attachment-name", "mid-length"]) {
      const copy = chip.cloneNode(true) as HTMLElement;
      copy.querySelector(".attachment-name-base")!.textContent = name;
      copy.dataset.clone = "true";
      chip.parentElement!.append(copy);
    }
  });
  const columns = new Map<number, number[]>();
  for (const box of await page.locator(".message-attachment").evaluateAll((chips) => chips.map((chip) => chip.getBoundingClientRect().toJSON() as DOMRect))) {
    const rights = columns.get(Math.round(box.left)) ?? [];
    rights.push(box.right);
    columns.set(Math.round(box.left), rights);
  }
  expect([...columns.values()].some((rights) => rights.length > 1)).toBe(true);
  for (const rights of columns.values()) expect(Math.max(...rights) - Math.min(...rights)).toBeLessThan(1);

  const chip = page.locator(".message-attachment:not([data-clone])");
  const chipBox = (await chip.boundingBox())!;
  const save = chip.getByRole("button", { name: "Save threestrands-shortcuts.txt as…" });
  await save.hover();
  const tooltip = chip.getByRole("tooltip").filter({ hasText: "Save threestrands-shortcuts.txt as…" });
  await expect(tooltip).toBeVisible();
  const tipBox = (await tooltip.boundingBox())!;
  // The tooltip hangs below the chip; nothing clips it to the chip's box.
  expect(tipBox.y).toBeGreaterThanOrEqual(chipBox.y + chipBox.height);
  expect(await tooltip.evaluate((element) => {
    const box = element.getBoundingClientRect();
    const clipping: string[] = [];
    for (let ancestor = element.parentElement; ancestor && ancestor !== document.body; ancestor = ancestor.parentElement) {
      const style = getComputedStyle(ancestor);
      if (style.overflowX === "visible" && style.overflowY === "visible") continue;
      if (/(auto|scroll)/.test(style.overflowY)) break;
      const clip = ancestor.getBoundingClientRect();
      if (box.left < clip.left || box.right > clip.right || box.top < clip.top || box.bottom > clip.bottom) clipping.push(ancestor.className);
    }
    return clipping;
  })).toEqual([]);
});


test("Files searches locally, stars by keyboard, and saves the selected attachment", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Files (5)" }).click();
  const files = page.getByRole("region", { name: "Files", exact: true });
  await expect(files.getByRole("listitem").first()).toBeVisible();
  await page.keyboard.press("/");
  const search = files.getByRole("searchbox", { name: "Search Files" });
  await expect(search).toBeFocused();
  await search.fill("threestrands-shortcuts.txt");
  const row = files.locator(".file-list-select").first();
  await row.focus();
  await page.keyboard.press("s");
  await expect(files.getByRole("complementary", { name: "File context" }).getByRole("button", { name: "Unstar threestrands-shortcuts.txt" })).toBeVisible();
  const download = page.waitForEvent("download");
  await page.keyboard.press("ControlOrMeta+Shift+s");
  expect((await download).suggestedFilename()).toBe("threestrands-shortcuts.txt");
  await page.keyboard.press("ControlOrMeta+k");
  const palette = page.getByRole("dialog", { name: "Command Palette" });
  await palette.getByRole("textbox", { name: "Filter Commands" }).fill("Search Current View");
  await palette.getByRole("button", { name: /Search Current View/ }).click();
  await expect(search).toBeFocused();
});
