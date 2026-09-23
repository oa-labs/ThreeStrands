import { expect, test } from "@playwright/test";

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
  await page.getByRole("button", { name: "Drafts (1)" }).click();
  await page.getByRole("button", { name: /Offline draft/ }).click();
  await expect(composer.getByRole("textbox", { name: "Message Body" })).toHaveText("Hello j k e r a f — these are text, not inbox actions.");
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect(composer).not.toBeVisible();
  await page.getByRole("button", { name: "Undo Send", exact: true }).click();
  await expect(composer.getByRole("textbox", { name: "Subject" })).toHaveValue("Offline draft");
  await expect(page.getByRole("button", { name: "Outbox (0)" })).toBeVisible();
});

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
  await expect(page.locator(".compose-backdrop")).toHaveCount(0);
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
  await page.getByRole("button", { name: /Outbox \(/ }).click();
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
