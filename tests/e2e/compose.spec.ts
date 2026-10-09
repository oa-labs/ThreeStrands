import { expect, test } from "@playwright/test";

async function openFolders(page: import("@playwright/test").Page) {
  await page.getByRole("button", { name: /Choose folder, current folder/ }).click();
  return page.getByRole("group", { name: "Folders" });
}

test("saves an offline draft, restores after reload, sends once, and undoes", async ({ page, context }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New Message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New Message" });
  await context.setOffline(true);
  await composer.getByRole("textbox", { name: "To", exact: true }).fill("friend@example.com");
  await composer.getByRole("textbox", { name: "Subject" }).fill("Offline draft");
  await composer.getByRole("textbox", { name: "Message Body" }).fill("Hello j k e r a f — these are text, not inbox actions.");
  await expect(composer.getByRole("status")).toHaveText("Saved on this device");
  await composer.getByRole("button", { name: "Save and Close Draft" }).click();
  await context.setOffline(false);
  await page.reload();
  const folders = await openFolders(page);
  await expect(folders.getByRole("button", { name: "Drafts" })).toContainText("1");
  await folders.getByRole("button", { name: "Drafts" }).click();
  await page.getByRole("button", { name: /Offline draft/ }).click();
  await expect(composer.getByRole("textbox", { name: "Message Body" })).toHaveText("Hello j k e r a f — these are text, not inbox actions.");
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect(composer).not.toBeVisible();
  await page.getByRole("button", { name: "Undo Send", exact: true }).click();
  await expect(composer.getByRole("textbox", { name: "Subject" })).toHaveValue("Offline draft");
  await expect((await openFolders(page)).getByRole("button", { name: "Outbox" }).locator(".folder-menu-meta > span")).toHaveCount(0);
});

for (const paste of [
  { subject: "Pasted link", text: "https://example.com/docs", expected: "Read the docs", linkedText: "Read the docs" },
  { subject: "Pasted text", text: '<img src=x onerror=alert(1)> https://example.com/docs', expected: '<img src=x onerror=alert(1)> https://example.com/docs', linkedText: "https://example.com/docs" },
]) {
  test(`${paste.subject} supports undo and redo and survives autosave and reopening`, async ({ page }) => {
    await page.goto("/");
    await page.getByRole("button", { name: "New Message (c)" }).click();
    const composer = page.getByRole("dialog", { name: "New Message" });
    const body = composer.getByRole("textbox", { name: "Message Body" });
    await composer.getByRole("textbox", { name: "Subject" }).fill(paste.subject);
    await body.fill("Read the docs");
    await body.press("ControlOrMeta+a");
    // Supply clipboard data without relying on the host clipboard. The editor
    // still executes the real browser editing command and uses its undo stack.
    await body.evaluate((editor, text) => {
      const clipboardData = new DataTransfer();
      clipboardData.setData("text/plain", text);
      editor.dispatchEvent(new ClipboardEvent("paste", { clipboardData, bubbles: true, cancelable: true }));
    }, paste.text);
    await expect(body).toHaveText(paste.expected);
    await expect(body.locator("a")).toHaveAttribute("href", "https://example.com/docs");
    await expect(body.locator("a")).toHaveText(paste.linkedText);
    await expect(body.locator("img, script")).toHaveCount(0);
    await expect(composer.getByRole("status")).toHaveText("Saved on this device");

    await body.press("ControlOrMeta+z");
    await expect(body).toHaveText("Read the docs");
    await expect(body.locator("a")).toHaveCount(0);
    await body.press("ControlOrMeta+Shift+z");
    await expect(body).toHaveText(paste.expected);
    await expect(body.locator("a")).toHaveAttribute("href", "https://example.com/docs");
    await expect(composer.getByRole("status")).toHaveText("Saved on this device");

    await composer.getByRole("button", { name: "Save and Close Draft" }).click();
    await expect(composer).not.toBeVisible();
    await page.reload();
    await (await openFolders(page)).getByRole("button", { name: "Drafts" }).click();
    await page.getByRole("button", { name: new RegExp(paste.subject) }).click();
    await expect(body).toHaveText(paste.expected);
    await expect(body.locator("a")).toHaveAttribute("href", "https://example.com/docs");
    await expect(body.locator("img, script")).toHaveCount(0);
  });
}

test("reply shortcuts keep inbox actions out of the composer and forwarding starts unaddressed", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.getByTitle("Message content").contentFrame().locator("body").click();
  await page.keyboard.press("r");
  const reply = page.getByRole("dialog", { name: "Reply Message" });
  await expect(reply).toBeVisible();
  await expect(page.getByRole("region", { name: "Conversation" }).getByRole("dialog", { name: "Reply Message" })).toBeVisible();
  await expect(reply.locator("xpath=parent::*")).toHaveClass(/message-stack/);
  await expect(reply.getByRole("button", { name: "Remove ThreeStrands" })).toBeVisible();
  const replyBody = reply.getByRole("textbox", { name: "Message Body" });
  await expect(replyBody).toHaveCSS("outline-style", "none");
  await expect(replyBody).toHaveCSS("padding-top", "12px");
  await replyBody.pressSequentially("jkeraf");
  await reply.getByRole("button", { name: "Attach Files" }).focus();
  await page.keyboard.press("e");
  await page.keyboard.press("Escape");
  await expect(reply).not.toBeVisible();
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();

  await page.getByTitle("Message content").contentFrame().locator("body").click();
  await page.keyboard.press("a");
  const replyAll = page.getByRole("dialog", { name: "Reply Message" });
  await expect(replyAll.getByRole("heading", { name: "Reply All" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(replyAll).not.toBeVisible();

  await page.getByRole("button", { name: "Forward" }).click();
  const forward = page.getByRole("dialog", { name: "Forward Message" });
  await expect(forward.getByRole("textbox", { name: "To", exact: true })).toHaveValue("");
  await expect(forward.getByRole("textbox", { name: "Subject" })).toHaveValue("Fwd: Welcome to ThreeStrands");
});

for (const [key, heading] of [["r", "Reply"], ["a", "Reply All"]] as const) {
  test(`${key} quotes only selected email text`, async ({ page }) => {
    await page.goto("/");
    await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
    const body = page.getByTitle("Message content").contentFrame().locator("body");
    await body.click();
    const selected = await body.evaluate((element) => {
      const walker = element.ownerDocument.createTreeWalker(element, NodeFilter.SHOW_TEXT);
      let node: Node | null;
      while ((node = walker.nextNode()) && !node.textContent?.trim()) { /* find visible text */ }
      if (!node) throw new Error("Message has no text to select");
      const range = element.ownerDocument.createRange();
      range.setStart(node, 0);
      range.setEnd(node, Math.min(12, node.textContent!.length));
      const selection = element.ownerDocument.defaultView!.getSelection()!;
      selection.removeAllRanges();
      selection.addRange(range);
      return selection.toString().trim();
    });
    await page.keyboard.press(key);

    const reply = page.getByRole("dialog", { name: "Reply Message" });
    await expect(reply.getByRole("heading", { name: heading, exact: true })).toBeVisible();
    const editor = reply.getByRole("textbox", { name: "Message Body" });
    await expect(editor).toBeFocused();
    await expect(editor).toHaveText("");
    const quoted = reply.locator('[aria-label="Quoted Text"]');
    await expect(quoted).toBeHidden();
    await reply.getByRole("button", { name: "Show Quoted Text" }).click();
    await expect(quoted).toBeVisible();
    await expect(quoted.locator('blockquote[type="cite"]')).toContainText(selected);
    await expect(quoted).not.toContainText("A keyboard-first inbox that keeps your mail on this device.");
  });
}

test("switching conversations saves an edited reply all and shows the selected thread", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.getByTitle("Message content").contentFrame().locator("body").click();
  await page.keyboard.press("a");

  const reply = page.getByRole("dialog", { name: "Reply Message" });
  await expect(reply.getByRole("heading", { name: "Reply All" })).toBeVisible();
  await reply.getByRole("textbox", { name: "Message Body" }).fill("Keep this reply all draft");
  await page.getByRole("listbox").getByRole("option", { name: /Phase 1: read and triage/ }).click();

  await expect(reply).toHaveCount(0);
  await expect(page.getByRole("region", { name: "Conversation" }).getByRole("heading", { name: "Phase 1: read and triage" })).toBeVisible();
  await (await openFolders(page)).getByRole("button", { name: /Drafts/ }).click();
  await page.getByRole("button", { name: /Welcome to ThreeStrands/ }).click();
  await expect(reply.getByRole("textbox", { name: "Message Body" })).toHaveText("Keep this reply all draft");
});

test("Reply Assist opens in the viewport over a long reply and Escape returns to compose", async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 600 });
  await page.goto("/");
  await page.evaluate(() => {
    localStorage.setItem("threestrands.settings.ai.provider", "openai");
    localStorage.setItem("threestrands.settings.ai.features", JSON.stringify({ draftAssist: true, summarize: false, actionExtraction: false }));
  });
  await page.evaluate('import("/src/aiSettings.ts").then(({ setAiApiKey }) => setAiApiKey("test-key"))');
  await page.getByTitle("Message content").contentFrame().locator("body").click();
  await page.keyboard.press("r");
  const reply = page.getByRole("dialog", { name: "Reply Message" });
  const editor = reply.getByRole("textbox", { name: "Message Body" });
  await editor.fill("A long quoted thread.\n".repeat(80));

  await editor.press("ControlOrMeta+j");

  const assist = page.getByRole("dialog", { name: "Reply Assist" });
  await expect(assist).toBeInViewport({ ratio: 1 });
  await expect(assist.getByRole("textbox", { name: "Optional Short Instruction" })).toBeFocused();
  const box = await assist.boundingBox();
  expect(box).not.toBeNull();
  expect(Math.abs(box!.y + box!.height / 2 - 300)).toBeLessThan(24);

  await page.keyboard.press("Escape");
  await expect(assist).not.toBeVisible();
  await expect(reply).toBeVisible();
  await expect(editor).toBeFocused();
});

test("removing an added reply recipient keeps the original recipient", async ({ page }) => {
  await page.goto("/");
  await page.getByTitle("Message content").contentFrame().locator("body").click();
  await page.keyboard.press("r");
  const reply = page.getByRole("dialog", { name: "Reply Message" });
  const to = reply.getByRole("textbox", { name: "To", exact: true });

  await to.fill("added@example.com");
  await to.press("Enter");
  await reply.getByRole("button", { name: "Remove added@example.com" }).click();

  await expect(reply.getByRole("button", { name: "Remove ThreeStrands" })).toBeVisible();
  await expect(reply.getByRole("button", { name: "Remove added@example.com" })).toHaveCount(0);
});

test("Superhuman formatting shortcuts edit rich compose content and appear in help", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New Message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New Message" });
  const body = composer.getByRole("textbox", { name: "Message Body" });

  await body.fill("Bold text");
  await body.press("ControlOrMeta+a");
  await body.press("ControlOrMeta+b");
  await expect(body.locator("b, strong")).toHaveText("Bold text");

  await body.fill("ThreeStrands");
  await body.press("ControlOrMeta+a");
  page.once("dialog", (dialog) => dialog.accept("https://threestrands.local"));
  await body.press("ControlOrMeta+k");
  await expect(body.locator("a")).toHaveAttribute("href", "https://threestrands.local");

  await body.fill("One");
  await body.press("ControlOrMeta+a");
  await body.press("ControlOrMeta+Shift+7");
  await expect(body.locator("ol > li")).toHaveText("One");

  await body.fill("Quoted");
  await body.press("ControlOrMeta+a");
  await body.press("ControlOrMeta+Shift+9");
  await expect(body.locator("blockquote")).toHaveText("Quoted");

  await composer.getByRole("button", { name: "Save and Close Draft" }).click();
  await page.keyboard.press("Shift+/");
  const help = page.getByRole("dialog", { name: "Keyboard Shortcuts" });
  await expect(help.getByText("Bold", { exact: true })).toBeVisible();
  await expect(help.getByText("Hyperlink", { exact: true })).toBeVisible();
  await expect(help.getByText("Decrease Indent", { exact: true })).toBeVisible();
});

test("attachment selection and removal survive autosave; invalid recipients keep the draft", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New Message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New Message" });
  await composer.getByRole("textbox", { name: "Subject" }).fill("Files");
  const picker = page.waitForEvent("filechooser");
  await composer.getByRole("button", { name: "Attach Files" }).click();
  await (await picker).setFiles({ name: "résumé.txt", mimeType: "text/plain", buffer: Buffer.from("attachment bytes") });
  await expect(composer.getByText("résumé.txt", { exact: false })).toBeVisible();
  await composer.getByRole("button", { name: "Remove résumé.txt" }).click();
  await expect(composer.getByText("résumé.txt", { exact: false })).not.toBeVisible();
  await composer.getByRole("button", { name: /^Send / }).click();
  await expect(composer.getByRole("alert")).toContainText("Add at least one recipient");
  await expect(composer).toBeVisible();
});

test("a new message opens as the conversation pane instead of a modal window", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.getByRole("button", { name: "New Message (c)" }).click();
  const reader = page.getByRole("region", { name: "Conversation" });
  const composer = page.getByRole("dialog", { name: "New Message" });
  await expect(reader.getByRole("dialog", { name: "New Message" })).toBeVisible();
  await expect(composer.locator("xpath=parent::*")).toHaveClass(/draft-message-stack/);
  await expect(reader.getByRole("heading", { name: "Welcome to ThreeStrands" })).toHaveCount(0);
  await expect(composer).not.toHaveAttribute("aria-modal", "true");
});

test("the command palette can send from a composer and the outbox records simulated delivery", async ({ page }) => {
  await page.clock.install();
  await page.goto("/");
  await page.getByRole("button", { name: "New Message (c)" }).click();
  await page.getByRole("textbox", { name: "To", exact: true }).fill("friend@example.com");
  await page.getByRole("textbox", { name: "Subject" }).fill("Palette send");
  await page.keyboard.press("ControlOrMeta+k");
  await page.getByRole("textbox", { name: "Filter Commands" }).fill("Send draft");
  await page.getByRole("button", { name: /Send Draft/ }).click();
  await expect(page.getByRole("dialog", { name: "New Message" })).not.toBeVisible();
  await page.clock.fastForward(11000);
  await (await openFolders(page)).getByRole("button", { name: "Outbox" }).click();
  await expect(page.getByRole("list", { name: "Outbox" })).toContainText("sent");
});

test("a recipient badge can be dragged from To into Cc", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New Message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New Message" });
  const to = composer.getByRole("textbox", { name: "To", exact: true });
  await to.fill("friend@example.com");
  await to.press("Enter");
  await composer.getByRole("button", { name: "To", exact: true }).click();

  const toRow = composer.getByRole("textbox", { name: "To", exact: true }).locator("xpath=..");
  const ccRow = composer.getByRole("textbox", { name: "Cc", exact: true }).locator("xpath=..");
  await expect(toRow.locator(".recipient-chip")).toHaveCount(1);

  await toRow.locator(".recipient-chip", { hasText: "friend@example.com" }).dragTo(ccRow);

  await expect(toRow.locator(".recipient-chip")).toHaveCount(0);
  await expect(ccRow.locator(".recipient-chip", { hasText: "friend@example.com" })).toHaveCount(1);
});

for (const theme of ["light", "dark"] as const) {
  test(`composer fits a small window in ${theme} mode`, async ({ page }, testInfo) => {
    await page.emulateMedia({ colorScheme: theme });
    await page.setViewportSize({ width: 900, height: 600 });
    await page.goto("/");
    await page.getByRole("button", { name: "New Message (c)" }).click();
    await page.getByRole("textbox", { name: "To", exact: true }).fill("Jane <jane@example.com>");
    await page.getByRole("textbox", { name: "Subject" }).fill("A quick update");
    await page.getByRole("textbox", { name: "Message Body" }).fill("Hi Jane,\n\nI've attached my notes from today. Let me know what you think.\n\nThanks!");
    await page.getByRole("button", { name: "To", exact: true }).click();
    await expect(page.getByRole("button", { name: /^Send / })).toBeInViewport();
    await expect(page.getByRole("button", { name: "Save and Close Draft" })).toBeInViewport();
    await expect(page.getByRole("dialog").getByRole("status")).toHaveText("Saved on this device");
    await page.screenshot({ path: testInfo.outputPath(`composer-${theme}.png`) });
  });
}

test("a mailto link in a message starts a prefilled draft in ThreeStrands", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  const frame = page.getByTitle("Message content").contentFrame();
  await frame.locator("body").evaluate((body) => {
    const link = body.ownerDocument.createElement("a");
    link.href = "mailto:jane@example.com?cc=alex@example.com&subject=Quarterly%20plan&body=Hi%20Jane%2C%0D%0ASee%20below.&attach=%2Fetc%2Fpasswd";
    link.textContent = "Write to Jane";
    body.append(link);
  });

  await frame.getByRole("link", { name: "Write to Jane" }).click();

  const composer = page.getByRole("dialog", { name: "New Message" });
  await expect(composer.getByRole("button", { name: "Remove jane@example.com" })).toBeVisible();
  await expect(composer.getByRole("button", { name: "Remove alex@example.com" })).toBeVisible();
  await expect(composer.getByRole("textbox", { name: "Subject" })).toHaveValue("Quarterly plan");
  await expect(composer.getByRole("textbox", { name: "Message Body" })).toContainText("Hi Jane,");
  await expect(composer.getByRole("textbox", { name: "Message Body" })).toContainText("See below.");
  await expect(composer.getByRole("button", { name: /^Remove .*passwd/ })).toHaveCount(0);
});
