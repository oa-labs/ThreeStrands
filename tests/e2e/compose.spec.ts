import { expect, test } from "@playwright/test";

test("saves an offline draft, restores after reload, sends once, and undoes", async ({ page, context }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New message" });
  await context.setOffline(true);
  await composer.getByRole("textbox", { name: "To", exact: true }).fill("friend@example.com");
  await composer.getByRole("textbox", { name: "Subject" }).fill("Offline draft");
  await composer.getByRole("textbox", { name: "Message body" }).fill("Hello j k e r a f — these are text, not inbox actions.");
  await expect(composer.getByRole("status")).toHaveText("Saved on this device");
  await composer.getByRole("button", { name: "Save and close draft" }).click();
  await context.setOffline(false);
  await page.reload();
  await page.getByRole("button", { name: "Drafts (1)" }).click();
  await page.getByRole("button", { name: /Offline draft/ }).click();
  await expect(composer.getByRole("textbox", { name: "Message body" })).toHaveText("Hello j k e r a f — these are text, not inbox actions.");
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect(composer).not.toBeVisible();
  await page.getByRole("button", { name: "Undo send", exact: true }).click();
  await expect(composer.getByRole("textbox", { name: "Subject" })).toHaveValue("Offline draft");
  await expect(page.getByRole("button", { name: "Outbox (0)" })).toBeVisible();
});

test("reply shortcuts keep inbox actions out of the composer and forwarding starts unaddressed", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to Dispatch" })).toBeVisible();
  await page.keyboard.press("r");
  const reply = page.getByRole("dialog", { name: "Reply message" });
  await expect(reply).toBeVisible();
  await expect(reply.getByRole("textbox", { name: "To", exact: true })).toHaveValue(/hello@dispatch.local/);
  await reply.getByRole("textbox", { name: "Message body" }).pressSequentially("jkeraf");
  await reply.getByRole("button", { name: "Attach files" }).focus();
  await page.keyboard.press("e");
  await page.keyboard.press("Escape");
  await expect(reply).not.toBeVisible();
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();

  await page.keyboard.press("a");
  const replyAll = page.getByRole("dialog", { name: "Reply message" });
  await expect(replyAll.getByRole("heading", { name: "Reply all" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(replyAll).not.toBeVisible();

  await page.getByRole("button", { name: "Forward (f)" }).click();
  const forward = page.getByRole("dialog", { name: "Forward message" });
  await expect(forward.getByRole("textbox", { name: "To", exact: true })).toHaveValue("");
  await expect(forward.getByRole("textbox", { name: "Subject" })).toHaveValue("Fwd: Welcome to Dispatch");
});

test("Superhuman formatting shortcuts edit rich compose content and appear in help", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New message" });
  const body = composer.getByRole("textbox", { name: "Message body" });

  await body.fill("Bold text");
  await body.press("ControlOrMeta+a");
  await body.press("ControlOrMeta+b");
  await expect(body.locator("b, strong")).toHaveText("Bold text");

  await body.fill("Dispatch");
  await body.press("ControlOrMeta+a");
  page.once("dialog", (dialog) => dialog.accept("https://dispatch.local"));
  await body.press("ControlOrMeta+k");
  await expect(body.locator("a")).toHaveAttribute("href", "https://dispatch.local");

  await body.fill("One");
  await body.press("ControlOrMeta+a");
  await body.press("ControlOrMeta+Shift+7");
  await expect(body.locator("ol > li")).toHaveText("One");

  await body.fill("Quoted");
  await body.press("ControlOrMeta+a");
  await body.press("ControlOrMeta+Shift+9");
  await expect(body.locator("blockquote")).toHaveText("Quoted");

  await composer.getByRole("button", { name: "Save and close draft" }).click();
  await page.getByRole("button", { name: "Keyboard shortcuts (?)" }).click();
  const help = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(help.getByText("Bold", { exact: true })).toBeVisible();
  await expect(help.getByText("Hyperlink", { exact: true })).toBeVisible();
  await expect(help.getByText("Decrease indent", { exact: true })).toBeVisible();
});

test("attachment selection and removal survive autosave; invalid recipients keep the draft", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New message" });
  await composer.getByRole("textbox", { name: "Subject" }).fill("Files");
  const picker = page.waitForEvent("filechooser");
  await composer.getByRole("button", { name: "Attach files" }).click();
  await (await picker).setFiles({ name: "résumé.txt", mimeType: "text/plain", buffer: Buffer.from("attachment bytes") });
  await expect(composer.getByText("résumé.txt", { exact: false })).toBeVisible();
  await composer.getByRole("button", { name: "Remove résumé.txt" }).click();
  await expect(composer.getByText("résumé.txt", { exact: false })).not.toBeVisible();
  await composer.getByRole("button", { name: /^Send / }).click();
  await expect(composer.getByRole("alert")).toContainText("Add at least one recipient");
  await expect(composer).toBeVisible();
});

test("composer size is resizable and restored after reload", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New message" });
  const handle = composer.getByRole("button", { name: "Resize compose window" });
  const body = composer.getByRole("textbox", { name: "Message body" });
  const initial = await composer.boundingBox();
  const initialBody = await body.boundingBox();
  const grip = await handle.boundingBox();
  expect(initial).not.toBeNull();
  expect(grip).not.toBeNull();
  expect(initialBody).not.toBeNull();

  await page.mouse.move(grip!.x + grip!.width / 2, grip!.y + grip!.height / 2);
  await page.mouse.down();
  await page.mouse.move(grip!.x + grip!.width / 2 + 80, grip!.y + grip!.height / 2 + 120);
  await page.mouse.up();

  const resized = await composer.boundingBox();
  const resizedBody = await body.boundingBox();
  const footer = await composer.getByRole("button", { name: /^Send / }).boundingBox();
  expect(resized!.width).toBeCloseTo(initial!.width + 80, 0);
  expect(resized!.height).toBeCloseTo(initial!.height + 120, 0);
  expect(resizedBody!.height).toBeGreaterThan(initialBody!.height + 40);
  expect(footer!.y - (resizedBody!.y + resizedBody!.height)).toBeLessThan(24);
  await composer.getByRole("button", { name: "Save and close draft" }).click();

  await page.reload();
  await page.getByRole("button", { name: "New message (c)" }).click();
  const restored = page.getByRole("dialog", { name: "New message" });
  const restoredBox = await restored.boundingBox();
  const restoredBody = await restored.getByRole("textbox", { name: "Message body" }).boundingBox();
  expect(restoredBox!.width).toBeCloseTo(resized!.width, 0);
  expect(restoredBox!.height).toBeCloseTo(resized!.height, 0);
  expect(restoredBody!.height).toBeCloseTo(resizedBody!.height, 0);
});

test("composer position is movable and restored after reload", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New message" });
  const title = composer.getByRole("heading", { name: "New message" });
  const initial = await composer.boundingBox();
  const handle = await title.boundingBox();
  expect(initial).not.toBeNull();
  expect(handle).not.toBeNull();

  await page.mouse.move(handle!.x + handle!.width / 2, handle!.y + handle!.height / 2);
  await page.mouse.down();
  await page.mouse.move(handle!.x + handle!.width / 2 - 120, handle!.y + handle!.height / 2 - 80);
  await page.mouse.up();

  const moved = await composer.boundingBox();
  expect(moved!.x).toBeCloseTo(initial!.x - 120, 0);
  expect(moved!.y).toBeCloseTo(initial!.y - 80, 0);
  await composer.getByRole("button", { name: "Save and close draft" }).click();

  await page.reload();
  await page.getByRole("button", { name: "New message (c)" }).click();
  const restored = await page.getByRole("dialog", { name: "New message" }).boundingBox();
  expect(restored!.x).toBeCloseTo(moved!.x, 0);
  expect(restored!.y).toBeCloseTo(moved!.y, 0);
});

test("the command palette can send from a composer and the outbox records simulated delivery", async ({ page }) => {
  await page.clock.install();
  await page.goto("/");
  await page.getByRole("button", { name: "New message (c)" }).click();
  await page.getByRole("textbox", { name: "To", exact: true }).fill("friend@example.com");
  await page.getByRole("textbox", { name: "Subject" }).fill("Palette send");
  await page.keyboard.press("ControlOrMeta+k");
  await page.getByRole("textbox", { name: "Filter commands" }).fill("Send draft");
  await page.getByRole("button", { name: /Send draft/ }).click();
  await expect(page.getByRole("dialog", { name: "New message" })).not.toBeVisible();
  await page.clock.fastForward(11000);
  await page.getByRole("button", { name: /Outbox \(/ }).click();
  await expect(page.getByRole("list", { name: "Outbox" })).toContainText("sent");
});

for (const theme of ["light", "dark"] as const) {
  test(`composer fits a small window in ${theme} mode`, async ({ page }, testInfo) => {
    await page.emulateMedia({ colorScheme: theme });
    await page.setViewportSize({ width: 900, height: 600 });
    await page.goto("/");
    await page.getByRole("button", { name: "New message (c)" }).click();
    await page.getByRole("textbox", { name: "To", exact: true }).fill("Jane <jane@example.com>");
    await page.getByRole("textbox", { name: "Subject" }).fill("A quick update");
    await page.getByRole("textbox", { name: "Message body" }).fill("Hi Jane,\n\nI've attached my notes from today. Let me know what you think.\n\nThanks!");
    await page.getByRole("button", { name: "Cc / Bcc" }).click();
    await expect(page.getByRole("button", { name: /^Send / })).toBeInViewport();
    await expect(page.getByRole("button", { name: "Save and close draft" })).toBeInViewport();
    await expect(page.getByRole("dialog").getByRole("status")).toHaveText("Saved on this device");
    await page.screenshot({ path: testInfo.outputPath(`composer-${theme}.png`) });
  });
}
