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
  await expect(composer.getByRole("textbox", { name: "Message body" })).toHaveValue("Hello j k e r a f — these are text, not inbox actions.");
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
  await page.getByRole("button", { name: "Forward (f)" }).click();
  const forward = page.getByRole("dialog", { name: "Forward message" });
  await expect(forward.getByRole("textbox", { name: "To", exact: true })).toHaveValue("");
  await expect(forward.getByRole("textbox", { name: "Subject" })).toHaveValue("Fwd: Welcome to Dispatch");
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
  await expect(page.getByRole("dialog", { name: "Outbox" })).toContainText("sent");
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
