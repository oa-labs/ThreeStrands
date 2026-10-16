import { expect, test } from "@playwright/test";

test("C opens a focused composer and autosave and polling preserve the body caret", async ({ page }) => {
  await page.clock.install();
  await page.goto("/");
  await expect(page.getByRole("button", { name: "New Message (c)" })).toBeVisible();
  await page.keyboard.press("c");
  const composer = page.getByRole("dialog", { name: "New Message" });
  await expect(composer.getByRole("textbox", { name: "To", exact: true })).toBeFocused();
  const body = composer.getByRole("textbox", { name: "Message Body" });
  await body.fill("Typed immediately after launch");
  await body.press("End");
  await page.clock.fastForward(10_000);
  await expect(body).toBeFocused();
  await expect(body).toHaveText("Typed immediately after launch");
  expect(await body.evaluate((element) => {
    const selection = window.getSelection();
    return Boolean(selection?.isCollapsed && element.contains(selection.anchorNode));
  })).toBe(true);
});

async function openFolders(page: import("@playwright/test").Page) {
  await page.getByRole("button", { name: /Choose folder, current folder/ }).click();
  return page.getByRole("group", { name: "Folders" });
}

async function enableDraftReview(page: import("@playwright/test").Page) {
  await page.evaluate(() => {
    localStorage.setItem("threestrands.settings.ai.provider", "openai");
    localStorage.setItem("threestrands.settings.ai.features", JSON.stringify({ draftAssist: true }));
  });
  await page.evaluate('import("/src/aiSettings.ts").then(({ setAiApiKey }) => setAiApiKey("test-key"))');
  // Closing settings refreshes availability without reopening the draft.
  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Close", exact: true }).click();
  await expect(settings).not.toBeVisible();
}

for (const name of [null, "A recipient with a very long display name that exceeds the panel width"]) {
  test(`long recipient ${name ? "names and email addresses" : "email addresses"} truncate within the context panel's person header`, async ({ page }) => {
    await page.goto("/");
    await page.getByRole("button", { name: "New Message (c)" }).click();
    const composer = page.getByRole("dialog", { name: "New Message" });
    const email = "someone.with.a.long.address@a.very.long.organization.example.com";
    const otherEmail = "another.recipient.with.a.long.address@a.very.long.organization.example.com";
    const to = composer.getByRole("textbox", { name: "To", exact: true });
    await to.fill(name ? `${name} <${email}>` : email);
    await to.press(",");
    await to.fill(otherEmail);
    await to.press(",");
    await composer.getByRole("textbox", { name: "Subject" }).focus();
    const panel = page.getByRole("complementary", { name: "Compose context" });
    const header = panel.getByRole("group", { name: "Showing history with" });
    // Unsaved recipients have no About section; the header names whose history is shown.
    await expect(panel.getByRole("region", { name: /^About / })).toHaveCount(0);
    const expectTruncated = async (line: ReturnType<typeof header.locator>, text: string) => {
      await expect(line).toBeVisible();
      await expect(line).toHaveText(text);
      await expect(line).toHaveCSS("white-space", "nowrap");
      await expect(line).toHaveCSS("text-overflow", "ellipsis");
      await expect(line).toHaveCSS("overflow-x", "hidden");
      const layout = await line.evaluate((element) => ({
        scrollWidth: element.scrollWidth,
        clientWidth: element.clientWidth,
        right: element.getBoundingClientRect().right,
        panelRight: element.closest(".context-panel")!.getBoundingClientRect().right,
      }));
      expect(layout.scrollWidth).toBeGreaterThan(layout.clientWidth);
      expect(layout.right).toBeLessThanOrEqual(layout.panelRight);
    };
    await expectTruncated(header.locator(".context-person-main .context-person-name"), name || email);
    if (name) await expectTruncated(header.locator(".context-person-main .context-person-email"), email);
    const toggle = header.getByRole("button", { name: "1 other", exact: true });
    await expect(toggle).toBeVisible();
    expect((await toggle.boundingBox())!.x + (await toggle.boundingBox())!.width)
      .toBeLessThanOrEqual((await panel.boundingBox())!.x + (await panel.boundingBox())!.width);

    await toggle.click();
    const others = header.getByRole("list", { name: "Other people" });
    await expectTruncated(others.locator(".context-person-name"), otherEmail);
    await others.getByRole("button").click();
    await expect(others).toHaveCount(0);
    await expect(header.locator(".context-person-main .context-person-name")).toHaveText(otherEmail);
    await header.getByRole("button", { name: "1 other", exact: true }).click();
    await expect(header.getByRole("list", { name: "Other people" }).locator(".context-person-name")).toHaveText(name || email);
  });
}

test("reviews a new email in the composer, applies and undoes without sending, and saves the original", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New Message (c)" }).click();
  await enableDraftReview(page);
  const composer = page.getByRole("dialog", { name: "New Message" });
  const panel = page.getByRole("region", { name: "Draft Review" });
  const body = composer.getByRole("textbox", { name: "Message Body" });
  await composer.getByRole("textbox", { name: "To", exact: true }).fill("client@example.com");
  await composer.getByRole("textbox", { name: "Subject" }).fill("An introduction");
  await body.fill("I help teams improve their workflow.");
  await composer.getByRole("button", { name: "Review", exact: true }).click();
  await panel.getByRole("textbox", { name: /Goal for This Email/ }).fill("Get an introductory call");
  await panel.getByRole("button", { name: "Review Draft", exact: true }).click();
  await expect(panel.locator("pre")).toHaveText("I help teams improve their workflow.");
  await expect(body).toHaveText("I help teams improve their workflow.");
  await panel.getByRole("button", { name: "Get Feedback" }).click();
  await panel.getByRole("button", { name: "Apply Revision" }).click();
  await expect(body).toContainText("Would you be open to a brief conversation?");
  await expect(composer.locator(".save-status")).toHaveText("Saved on this device");
  await panel.getByRole("button", { name: "Undo Revision" }).click();
  await expect(body).toHaveText("I help teams improve their workflow.");
  await composer.getByRole("button", { name: "Save and Close Draft" }).click();
  await (await openFolders(page)).getByRole("button", { name: /Drafts/ }).click();
  await page.getByRole("button", { name: /An introduction/ }).click();
  await expect(body).toHaveText("I help teams improve their workflow.");
  // A reopened draft starts with Draft Review closed and empty.
  await expect(panel).toHaveCount(0);
  await composer.getByRole("button", { name: "Review", exact: true }).click();
  await expect(panel.getByRole("textbox", { name: /Goal for This Email/ })).toHaveValue("");
  await expect(panel.getByRole("button", { name: "Apply Revision" })).toHaveCount(0);
});

test("draft review protects changes made after requesting feedback", async ({ page }) => {
  await page.goto("/");
  await enableDraftReview(page);
  await page.getByRole("button", { name: "New Message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New Message" });
  const panel = page.getByRole("region", { name: "Draft Review" });
  const body = composer.getByRole("textbox", { name: "Message Body" });
  await body.fill("Let me know your thoughts.");
  await composer.getByRole("button", { name: "Review", exact: true }).click();
  await panel.getByRole("button", { name: "Review Draft", exact: true }).click();
  await panel.getByRole("button", { name: "Get Feedback" }).click();
  await body.fill("Keep this newer wording.");
  await panel.getByRole("button", { name: "Apply Revision" }).click();
  await expect(panel.getByRole("alert")).toContainText("Your draft changed");
  await expect(body).toHaveText("Keep this newer wording.");
});

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
  await page.getByLabel("Message content", { exact: true }).contentFrame().locator("body").click();
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

  await page.getByLabel("Message content", { exact: true }).contentFrame().locator("body").click();
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

test("HTML forwarding preserves formatting and blocked images after reopening, sending, and undo", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Forward", exact: true }).click();
  const composer = page.getByRole("dialog", { name: "Forward Message" });
  const preview = composer.getByRole("region", { name: "Forwarded message" });
  const frame = preview.getByLabel("Message content", { exact: true }).contentFrame();
  await expect(frame.locator("strong")).toHaveText("ThreeStrands");
  await expect(frame.locator("img")).not.toHaveAttribute("src");
  await expect(preview.getByRole("button", { name: "Load images" })).toBeVisible();
  const body = composer.getByRole("textbox", { name: "Message Body" });
  await expect(body).toBeEmpty();
  await body.fill("Please see the original message below.");
  await composer.getByRole("textbox", { name: "To", exact: true }).fill("friend@example.com");
  await composer.getByRole("button", { name: "Save and Close Draft" }).click();
  await page.reload();
  await (await openFolders(page)).getByRole("button", { name: "Drafts" }).click();
  await page.getByRole("button", { name: /Fwd: Welcome to ThreeStrands/ }).click();
  await expect(body).toHaveText("Please see the original message below.");
  await expect(frame.locator("strong")).toHaveText("ThreeStrands");
  await expect(frame.locator("img")).not.toHaveAttribute("src");
  await composer.getByRole("button", { name: "Send", exact: true }).click();
  await expect(composer).not.toBeVisible();
  await page.getByRole("button", { name: "Undo Send", exact: true }).click();
  await expect(body).toHaveText("Please see the original message below.");
  await expect(frame.locator("strong")).toHaveText("ThreeStrands");
  await expect(frame.locator("img")).not.toHaveAttribute("src");
});

for (const [key, heading] of [["r", "Reply"], ["a", "Reply All"], ["f", "Forward"], [null, "Reply"], [null, "Reply All"], [null, "Forward"]] as const) {
  test(`${key ?? `${heading} icon`} quotes only selected email text`, async ({ page }) => {
    await page.goto("/");
    await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
    const body = page.getByLabel("Message content", { exact: true }).contentFrame().locator("body");
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
    if (key) await page.keyboard.press(key);
    else await page.getByRole("button", { name: heading, exact: true }).click();

    const reply = page.getByRole("dialog", { name: heading === "Forward" ? "Forward Message" : "Reply Message" });
    await expect(reply.getByRole("heading", { name: heading, exact: true })).toBeVisible();
    const editor = reply.getByRole("textbox", { name: "Message Body" });
    await expect(editor).toHaveText("");
    if (heading === "Forward") {
      const forwarded = reply.getByRole("region", { name: "Forwarded message" }).getByLabel("Message content", { exact: true }).contentFrame();
      await expect(forwarded.locator("blockquote")).toHaveText(selected);
      await expect(forwarded.locator("body")).not.toContainText("A keyboard-first inbox that keeps your mail on this device.");
      await reply.getByRole("button", { name: "Save and Close Draft" }).click();
      await (await openFolders(page)).getByRole("button", { name: /Drafts/ }).click();
      await page.getByRole("button", { name: /Fwd: Welcome to ThreeStrands/ }).click();
      await expect(forwarded.locator("blockquote")).toHaveText(selected);
      return;
    }
    await expect(editor).toBeFocused();
    const quoted = reply.locator('[aria-label="Quoted Text"]');
    await expect(quoted).toBeVisible();
    await expect(quoted.locator('blockquote[type="cite"]')).toContainText(selected);
    await expect(quoted).not.toContainText("A keyboard-first inbox that keeps your mail on this device.");
  });
}

test("switching conversations saves an edited reply all and shows the selected thread", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.getByLabel("Message content", { exact: true }).contentFrame().locator("body").click();
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

for (const width of [900, 1280]) {
  test(`Reply Assist opens in the viewport over a long reply and Escape returns to compose (${width}px)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 600 });
    await page.goto("/");
    await page.evaluate(() => {
      localStorage.setItem("threestrands.settings.ai.provider", "openai");
      localStorage.setItem("threestrands.settings.ai.features", JSON.stringify({ draftAssist: true, summarize: false, actionExtraction: false }));
    });
    // The app reads AI settings at startup (and again when Settings closes), as Draft Reply is offered only when it's on.
    await page.reload();
    await page.evaluate('import("/src/aiSettings.ts").then(({ setAiApiKey }) => setAiApiKey("test-key"))');
    await page.getByLabel("Message content", { exact: true }).contentFrame().locator("body").click();
    await page.keyboard.press("r");
    const reply = page.getByRole("dialog", { name: "Reply Message" });
    const editor = reply.getByRole("textbox", { name: "Message Body" });
    await editor.fill("A long quoted thread.\n".repeat(80));
    const tools = reply.getByRole("group", { name: "Writing tools" });
    const draftReply = tools.getByRole("button", { name: "Draft Reply", exact: true });
    await expect(draftReply).toHaveAttribute("title", /Draft a reply with AI/);
    await expect(draftReply.locator("kbd")).toHaveCount(0);
    await tools.scrollIntoViewIfNeeded();
    await expect(tools.getByRole("combobox", { name: "Signature" })).toHaveCount(0);
    await expect(draftReply).toBeInViewport();
    const review = tools.getByRole("button", { name: "Review", exact: true });
    await expect(review).toBeInViewport();
    const alignment = await tools.evaluate((element) => {
      const row = element.parentElement!;
      const buttons = Array.from(element.querySelectorAll("button"));
      const ai = buttons[0].getBoundingClientRect();
      const review = buttons.at(-1)!.getBoundingClientRect();
      const toggle = row.querySelector(".compose-quote-toggle")!.getBoundingClientRect();
      const composer = element.closest(".composer")!;
      const trashButton = composer.querySelector('[aria-label="Discard Draft"]')!;
      const trash = trashButton.getBoundingClientRect();
      const trashIcon = trashButton.querySelector("svg")!.getBoundingClientRect();
      const attach = composer.querySelector('[aria-label="Attach Files"]')!.getBoundingClientRect();
      const saved = composer.querySelector('[role="status"]')!.getBoundingClientRect();
      return {
        scrollWidth: row.scrollWidth, clientWidth: row.clientWidth,
        aiCenter: ai.y + ai.height / 2, reviewCenter: review.y + review.height / 2, toggleCenter: toggle.y + toggle.height / 2,
        reviewRight: review.right, trashIconRight: trashIcon.right,
        trashWidth: trash.width, trashHeight: trash.height, attachWidth: attach.width, attachHeight: attach.height,
        savedRight: saved.right, trashLeft: trash.left, trashRight: trash.right, composerRight: composer.getBoundingClientRect().right,
        toolsBorder: getComputedStyle(element).borderTopWidth, rowBorder: getComputedStyle(row).borderTopWidth,
        toolsBottom: element.getBoundingClientRect().bottom, footerTop: trash.top,
      };
    });
    expect(alignment.scrollWidth).toBeLessThanOrEqual(alignment.clientWidth);
    expect(Math.abs(alignment.reviewRight - alignment.trashIconRight)).toBeLessThan(1);
    expect(alignment.trashWidth).toBe(alignment.attachWidth);
    expect(alignment.trashHeight).toBe(alignment.attachHeight);
    expect(alignment.savedRight).toBeLessThanOrEqual(alignment.trashLeft);
    expect(alignment.trashRight).toBeLessThan(alignment.composerRight);
    if (width === 1280) {
      expect(Math.abs(alignment.aiCenter - alignment.toggleCenter)).toBeLessThan(1);
      expect(Math.abs(alignment.reviewCenter - alignment.toggleCenter)).toBeLessThan(1);
    }
    expect(alignment.toolsBorder).toBe("0px");
    expect(alignment.rowBorder).toBe("0px");
    expect(alignment.toolsBottom).toBeLessThanOrEqual(alignment.footerTop);

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
}

test("reply send options support keyboard navigation and send-and-archive without losing draft edits", async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 600 });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.getByLabel("Message content", { exact: true }).contentFrame().locator("body").click();
  await page.keyboard.press("r");
  const reply = page.getByRole("dialog", { name: "Reply Message" });
  const body = reply.getByRole("textbox", { name: "Message Body" });
  await body.fill("Thanks for the introduction.");
  const trigger = reply.getByRole("button", { name: "Send options" });
  await trigger.focus();
  await trigger.press("ArrowDown");
  const menu = reply.getByRole("menu", { name: "Send options" });
  const later = menu.getByRole("menuitem", { name: "Send later…" });
  const archive = menu.getByRole("menuitem", { name: "Send & archive" });
  await expect(later).toBeFocused();
  await page.keyboard.press("End");
  await expect(archive).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(menu).not.toBeVisible();
  await expect(trigger).toBeFocused();
  await expect(body).toHaveText("Thanks for the introduction.");
  await trigger.click();
  await page.keyboard.press("Tab");
  await expect(menu).not.toBeVisible();
  await expect(reply.getByRole("button", { name: "Attach Files" })).toBeFocused();
  await trigger.click();
  await expect(archive).toBeInViewport({ ratio: 1 });
  const menuFits = await menu.evaluate((element) => {
    const menuBox = element.getBoundingClientRect();
    const composerBox = element.closest(".composer")!.getBoundingClientRect();
    return menuBox.left >= composerBox.left && menuBox.right <= composerBox.right && element.scrollWidth <= element.clientWidth;
  });
  expect(menuFits).toBe(true);
  await later.focus();
  await page.keyboard.press("ControlOrMeta+Shift+Enter");
  await expect(reply).not.toBeVisible();
  await expect(page.getByRole("listbox").getByRole("option").filter({ hasText: "Welcome to ThreeStrands" })).toHaveCount(0);
  await page.getByRole("button", { name: "Undo Send", exact: true }).click();
  await expect(body).toHaveText("Thanks for the introduction.");
});

test("removing an added reply recipient keeps the original recipient", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("Message content", { exact: true }).contentFrame().locator("body").click();
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

  await body.fill("Italic text");
  await body.press("ControlOrMeta+a");
  await body.press("ControlOrMeta+i");
  await expect(body.locator("i, em")).toHaveText("Italic text");

  await body.fill("Underlined text");
  await body.press("ControlOrMeta+a");
  await body.press("ControlOrMeta+u");
  await expect(body.locator("u")).toHaveText("Underlined text");

  await body.fill("ThreeStrands");
  await body.press("ControlOrMeta+a");
  await body.press("ControlOrMeta+k");
  const linkDialog = page.getByRole("dialog", { name: "Insert Link" });
  await linkDialog.getByRole("textbox", { name: "Link URL" }).fill("threestrands.local");
  await linkDialog.getByRole("button", { name: "Insert Link" }).click();
  await expect(linkDialog).toHaveCount(0);
  await expect(body.locator("a")).toHaveAttribute("href", "https://threestrands.local");
  await expect(body.locator("a")).toHaveText("ThreeStrands");

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
  await composer.getByRole("button", { name: "Send", exact: true }).click();
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
  await (await openFolders(page)).getByRole("button", { name: "Outbox" }).click();
  await expect(page.getByRole("listbox", { name: "Outbox" })).toContainText("Undo available");
  // The Outbox reads like any folder: the selected send opens in the reader.
  const reader = page.getByRole("region", { name: "Outgoing message" });
  await expect(reader.getByRole("heading", { name: "Palette send" })).toBeVisible();
  await expect(reader).toContainText("From demo@example.com");
  await expect(reader.getByRole("button", { name: "Undo Send", exact: true })).toBeVisible();
  // Once delivered, the message leaves the Outbox; Sent is where it lives.
  await page.clock.fastForward(11000);
  await (await openFolders(page)).getByRole("button", { name: "Outbox" }).click();
  await expect(page.getByRole("listbox", { name: "Outbox" })).toContainText("No outgoing messages.");
  await expect(page.getByRole("region", { name: "Outgoing message" })).toHaveCount(0);
});

test("Sent is a folder in the folder menu and on g then s", async ({ page }) => {
  await page.goto("/");
  await (await openFolders(page)).getByRole("button", { name: /Sent/ }).click();
  await expect(page.getByRole("button", { name: "Choose folder, current folder Sent" })).toBeVisible();
  await expect(page.getByRole("listbox", { name: "Sent" })).toBeVisible();
  await page.keyboard.press("g");
  await page.keyboard.press("i");
  await expect(page.getByRole("button", { name: "Choose folder, current folder Inbox" })).toBeVisible();
  await page.keyboard.press("g");
  await page.keyboard.press("s");
  await expect(page.getByRole("listbox", { name: "Sent" })).toBeVisible();
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
    await expect(page.getByRole("button", { name: "Send", exact: true })).toBeInViewport();
    await expect(page.getByRole("button", { name: "Save and Close Draft" })).toBeInViewport();
    await expect(page.getByRole("dialog").getByRole("status")).toHaveText("Saved on this device");
    await page.screenshot({ path: testInfo.outputPath(`composer-${theme}.png`) });
  });
}

test("a mailto link in a message starts a prefilled draft in ThreeStrands", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  const frame = page.getByLabel("Message content", { exact: true }).contentFrame();
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

test("scheduled send survives reload, reschedules, and requires confirmation after a missed time", async ({ page }) => {
  await page.clock.install({ time: new Date("2026-10-10T12:00:00Z") });
  await page.goto("/");
  await page.getByRole("button", { name: "New Message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New Message" });
  await composer.getByRole("textbox", { name: "To", exact: true }).fill("test@example.com");
  await composer.getByRole("textbox", { name: "Subject" }).fill("A scheduled email");
  await composer.getByRole("textbox", { name: "Message Body" }).fill("Preserved scheduled content");
  await composer.getByRole("button", { name: "Send options" }).click();
  await expect(composer.getByRole("menuitem")).toHaveCount(1);
  await expect(composer.getByRole("menuitem", { name: "Send later…", exact: true })).toHaveAttribute("aria-keyshortcuts", "Meta+Shift+L Control+Shift+L");
  await composer.getByRole("menuitem", { name: "Send later…" }).click();
  const picker = page.getByRole("dialog", { name: "Send later" });
  await expect(picker).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(picker).not.toBeVisible();
  await expect(composer.getByRole("textbox", { name: "Message Body" })).toHaveText("Preserved scheduled content");
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("threestrands.demoCorrespondence")!).outbox)).toHaveLength(0);
  await page.keyboard.press("ControlOrMeta+Shift+L");
  await expect(picker).toBeVisible();
  await picker.getByLabel("Schedule timezone").fill("UTC");
  await picker.getByLabel("Scheduled date and time").fill("2026-10-11T09:00");
  await picker.getByRole("button", { name: "Schedule email" }).click();
  await expect(composer).not.toBeVisible();
  await page.reload();
  await (await openFolders(page)).getByRole("button", { name: /Outbox/ }).click();
  const item = page.getByRole("option").filter({ hasText: "A scheduled email" });
  await expect(item).toContainText("scheduled");
  await item.click();
  const detail = page.getByRole("region", { name: "Outgoing message" });
  await expect(detail.getByLabel("Message content", { exact: true }).contentFrame().locator("body")).toContainText("Preserved scheduled content");
  await detail.getByRole("button", { name: "Reschedule", exact: true }).click();
  const reschedule = page.getByRole("dialog", { name: "Reschedule email" });
  await reschedule.getByLabel("Scheduled date and time").fill("2026-10-12T09:00");
  await reschedule.getByRole("button", { name: "Save schedule" }).click();
  await page.clock.setSystemTime(new Date("2026-10-12T09:01:01Z"));
  await page.reload();
  await (await openFolders(page)).getByRole("button", { name: /Outbox/ }).click();
  await expect(item).toContainText("Scheduled time missed");
  await expect(detail).toContainText("Scheduled time missed");
  await expect(detail.getByRole("button", { name: "Send now", exact: true })).toBeVisible();
  await detail.getByRole("button", { name: "Send now", exact: true }).click();
  await detail.getByRole("button", { name: "Undo Send", exact: true }).click();
  await expect(composer.getByRole("textbox", { name: "Message Body" })).toHaveText("Preserved scheduled content");
});

test("peer scheduled visibility cannot send or restore message content", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("threestrands.demoCorrespondence", JSON.stringify({ drafts: [], outbox: [], summaries: [{
      operationId: "peer-job", ownerInstallationId: "peer-owner", ownerSyncDeviceId: null,
      ownerNameAtCreation: "MacBook", account: "me@example.com", subject: "Peer scheduled email",
      scheduledAt: 1, timeZone: "UTC", state: "scheduled", blockedReason: null,
      reportRevision: 1, statusChangedAt: 1,
    }] }));
  });
  await page.goto("/");
  await (await openFolders(page)).getByRole("button", { name: /Outbox/ }).click();
  const item = page.getByRole("option").filter({ hasText: "Peer scheduled email" });
  await expect(item).toContainText("Awaiting an update from MacBook");
  await expect(item).not.toContainText("@example.com>");
  const detail = page.getByRole("region", { name: "Outgoing message" });
  await expect(detail).toContainText("Scheduled time passed; awaiting an update from MacBook");
  await expect(detail).toContainText("Manage on MacBook");
  await expect(detail.getByRole("button")).toHaveCount(0);
});

test("the composer shows a sending-address picker for a multi-identity account and records the choice", async ({ page }) => {
  await page.goto("/");
  // Make the demo account report two sending identities, using the existing
  // demo mail client (no new mock framework) — the browser-preview account is
  // normally single-identity, which hides the picker.
  await page.evaluate(`import("/src/data/client.ts").then(({ mailClient }) => {
    mailClient.listIdentities = async () => [
      { address: "demo@example.com" },
      { address: "work@example.com", displayName: "Work" },
    ];
    window.__lastSetFrom = undefined;
    mailClient.setDraftFrom = async (id, from) => {
      window.__lastSetFrom = from;
      const drafts = await mailClient.listDrafts();
      const draft = drafts.find((d) => d.id === id) ?? drafts[0];
      return { ...draft, from, revision: draft.revision + 1 };
    };
  })`);
  await page.getByRole("button", { name: "New Message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New Message" });
  const picker = composer.getByRole("combobox", { name: "Sending address" });
  await expect(picker).toBeVisible();
  // Native <option>s are not "visible" while the select is closed; assert the
  // option exists and that selecting it drives the backend call instead.
  await expect(picker.locator("option")).toHaveCount(2);
  await picker.selectOption("work@example.com");
  // The chosen identity reached setDraftFrom.
  await expect.poll(() => page.evaluate(() => (window as unknown as { __lastSetFrom?: string }).__lastSetFrom)).toBe("work@example.com");
});
