import { expect, test } from "@playwright/test";

test("processes the inbox from the keyboard", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();

  await page.keyboard.press("s");
  await expect(page.getByRole("button", { name: "Unstar (s)" })).toBeVisible();

  await page.keyboard.press("j");
  await expect(page.getByRole("heading", { name: "Phase 1: read and triage" })).toBeVisible();

  await page.keyboard.press("e");
  await expect(page.getByRole("status")).toContainText("Conversation archived");
  await expect(page.getByRole("heading", { name: "2 conversations" })).toBeVisible();
  await expect(page.getByRole("option", { selected: true })).toContainText("Your inbox stays local");
  await expect(page.getByRole("heading", { name: "Your inbox stays local" })).toBeVisible();

  await expect(page.getByRole("status")).toBeHidden({ timeout: 10_000 });
});

test("keeps an email address popover open while moving to its copy button", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: async () => undefined },
    });
  });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();

  const address = page.locator(".message-sender-row .address").first();
  const popover = address.locator(".address-popover");
  const copy = address.getByRole("button", { name: "Copy hello@threestrands.local" });

  await address.hover();
  await expect(popover).toBeVisible();

  const addressBox = await address.boundingBox();
  const popoverBox = await popover.boundingBox();
  expect(addressBox).not.toBeNull();
  expect(popoverBox).not.toBeNull();
  if (!addressBox || !popoverBox) return;

  await page.mouse.move(popoverBox.x + 4, (addressBox.y + addressBox.height + popoverBox.y) / 2);
  await expect(popover).toBeVisible();
  await copy.hover();
  await copy.click();
  await expect(address.getByRole("button", { name: "Copied" })).toBeVisible();
});

test("confirms unsubscribe with Cmd/Ctrl+U when the message advertises one-click support", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Unsubscribe (⌘U)" })).toBeVisible();

  await page.keyboard.press("ControlOrMeta+u");
  const dialog = page.getByRole("dialog", { name: "Unsubscribe" });
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText("threestrands.example");
  await expect(dialog).toContainText("one-click request");

  await dialog.getByRole("button", { name: "Send one-click request" }).click();
  await expect(page.getByRole("status")).toContainText("Unsubscribe request sent");
  await expect(dialog).not.toBeVisible();
});

test("searches and opens the command palette", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("textbox", { name: "Search mail" })).toHaveCount(0);
  await expect(page.locator(".thread-header").getByRole("button", { name: "Filters" })).toBeVisible();
  await expect(page.locator(".sidebar").getByRole("button", { name: "Refresh mail" })).toBeVisible();
  await page.keyboard.press("/");
  const search = page.getByRole("textbox", { name: "Search mail" });
  await expect(search).toBeFocused();
  await search.fill("SQLite");
  await expect(page.getByRole("option")).toHaveCount(1);
  await expect(page.getByRole("heading", { name: "Your inbox stays local" })).toBeVisible();

  await page.keyboard.press("ControlOrMeta+k");
  const palette = page.getByRole("dialog", { name: "Command palette" });
  await expect(palette).toBeVisible();
  await expect(palette.getByRole("button", { name: /Increase font size/ })).toContainText("Mod+=");
  await expect(palette.getByRole("button", { name: /Increase font size/ })).toContainText("Mod++");
  await expect(palette.getByRole("button", { name: /Decrease font size/ })).toContainText("Mod+-");
});

test("dismisses search with Escape", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.keyboard.press("/");
  const search = page.getByRole("textbox", { name: "Search mail" });
  await search.fill("SQLite");
  await expect(page.getByRole("option")).toHaveCount(1);

  await page.keyboard.press("Escape");
  await expect(search).toHaveCount(0);
  await expect(page.getByRole("option")).toHaveCount(3);
});

test("changes the app font size with desktop shortcuts and restores it", async ({ page }) => {
  await page.goto("/");
  const messageBody = page.frameLocator(".message-body").locator("body");
  await expect(messageBody).toHaveCSS("font-size", "15px");

  await page.keyboard.press("ControlOrMeta+=");
  await expect(messageBody).toHaveCSS("font-size", "16.5px");
  await expect.poll(() => page.evaluate(() => localStorage.getItem("threestrands.fontScale"))).toBe("110");

  await page.reload();
  await expect(messageBody).toHaveCSS("font-size", "16.5px");

  await page.keyboard.press("ControlOrMeta+Shift+=");
  await expect(messageBody).toHaveCSS("font-size", "18px");
  await page.keyboard.press("ControlOrMeta+-");
  await expect(messageBody).toHaveCSS("font-size", "16.5px");
});

test("shows and dismisses dedicated keyboard shortcut help", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();

  await page.keyboard.press("Shift+/");
  const help = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(help).toBeVisible();
  await expect(help.getByRole("heading", { name: "Navigation" })).toBeVisible();
  await expect(help).toContainText("Go to Inbox");
  await expect(help).toContainText("Manage labels");
  await expect(help).toContainText("New Message");
  await expect(help).toContainText("Command palette");
  await expect(help).toContainText("Undo last action");

  await page.keyboard.press("Escape");
  await expect(help).not.toBeVisible();

  await page.keyboard.press("/");
  const search = page.getByRole("textbox", { name: "Search mail" });
  await search.focus();
  await page.keyboard.type("?");
  await expect(search).toHaveValue("?");
  await expect(help).not.toBeVisible();

  await page.getByRole("button", { name: "Keyboard shortcuts (?)" }).click();
  await expect(help).toBeVisible();
  await page.locator(".modal-backdrop").click({ position: { x: 5, y: 5 } });
  await expect(help).not.toBeVisible();
});

test("switches accounts from the keyboard and palette, and disconnecting one leaves the other unaffected", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();

  // Connect a second account from Settings → Accounts.
  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await expect(settings).toBeVisible();
  await settings.getByRole("button", { name: "Accounts", exact: true }).click();
  await settings.getByRole("button", { name: "Add account" }).click();
  await expect(settings.locator(".accounts-list li")).toHaveCount(2);
  await page.keyboard.press("Escape");
  await expect(settings).not.toBeVisible();

  // The sidebar shows a filter icon per account once a second account exists.
  const rail = page.getByRole("radiogroup", { name: "Filter by account" });
  await expect(rail).toBeVisible();

  // Cmd/Ctrl+2 scopes to the new (empty) account; Cmd/Ctrl+1 returns to the first.
  await page.keyboard.press("ControlOrMeta+2");
  await expect(page.getByRole("heading", { name: "0 conversations" })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+0");
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+2");
  await expect(page.getByRole("heading", { name: "0 conversations" })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+1");
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();

  // The command palette offers the same switching, discoverable like any other command.
  await page.keyboard.press("ControlOrMeta+k");
  let palette = page.getByRole("dialog", { name: "Command palette" });
  await palette.getByRole("button", { name: /Switch to demo-2@example.com/ }).click();
  await expect(page.getByRole("heading", { name: "0 conversations" })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+k");
  palette = page.getByRole("dialog", { name: "Command palette" });
  await palette.getByRole("button", { name: /Show all accounts/ }).click();
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();

  // A new message offers a From selector once more than one account is connected.
  await page.keyboard.press("c");
  const composer = page.getByRole("dialog", { name: "New Message" });
  const from = composer.getByRole("combobox", { name: "Send from" });
  await expect(from).toHaveValue("demo@example.com");
  await from.selectOption("demo-2@example.com");
  await expect(from).toHaveValue("demo-2@example.com");
  await composer.getByRole("button", { name: "Discard draft" }).click();
  await expect(composer).not.toBeVisible();

  // Shortcut help documents the new per-account bindings.
  await page.keyboard.press("Shift+/");
  const help = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(help).toContainText("Switch to demo@example.com");
  await expect(help).toContainText("Switch to demo-2@example.com");
  await expect(help).toContainText("Show all accounts");
  await page.keyboard.press("Escape");
  await expect(help).not.toBeVisible();

  // The sidebar rail lists both accounts and can switch back to "All accounts", still showing per-thread account dots.
  await expect(rail.getByRole("radio", { name: /demo-2@example.com/ })).toBeVisible();
  await rail.getByRole("radio", { name: "All accounts" }).click();
  await expect(page.locator(".thread-row .account-dot").first()).toBeVisible();

  // Removing the second account leaves the first one's shortcuts and inbox unaffected.
  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  await expect(settings).toBeVisible();
  await settings.getByRole("button", { name: "Accounts", exact: true }).click();
  await settings.locator("li", { hasText: "demo-2@example.com" }).getByRole("button", { name: "Disconnect" }).click();
  await expect(settings.locator(".accounts-list li")).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(settings).not.toBeVisible();
  await expect(rail).toBeHidden();
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();
  await page.keyboard.press("j");
  await expect(page.getByRole("heading", { name: "Phase 1: read and triage" })).toBeVisible();
});

test("saves an independent sender name for each account", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Accounts", exact: true }).click();
  await settings.getByRole("button", { name: "Add account" }).click();

  const personalName = settings.getByRole("textbox", { name: "Sender name for demo@example.com" });
  await personalName.fill("Joel Reed");
  await settings.locator(".account-card").filter({ hasText: "demo@example.com" }).getByRole("button", { name: "Save name" }).click();

  const workName = settings.getByRole("textbox", { name: "Sender name for demo-2@example.com" });
  await workName.fill("Joel at Work");
  await settings.locator(".account-card").filter({ hasText: "demo-2@example.com" }).getByRole("button", { name: "Save name" }).click();

  await expect(personalName).toHaveValue("Joel Reed");
  await expect(workName).toHaveValue("Joel at Work");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  await settings.getByRole("button", { name: "Accounts", exact: true }).click();
  await expect(settings.getByRole("textbox", { name: "Sender name for demo@example.com" })).toHaveValue("Joel Reed");
  await expect(settings.getByRole("textbox", { name: "Sender name for demo-2@example.com" })).toHaveValue("Joel at Work");
});

test("reorders navbar accounts by dragging their icons", async ({ page }) => {
  await page.goto("/");

  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Accounts", exact: true }).click();
  await settings.getByRole("button", { name: "Add account" }).click();
  await page.keyboard.press("Escape");

  const rail = page.getByRole("radiogroup", { name: "Filter by account" });
  const personal = rail.getByRole("radio", { name: "demo@example.com" });
  const work = rail.getByRole("radio", { name: "demo-2@example.com" });
  await expect(rail.getByRole("radio")).toHaveCount(3);

  await work.dragTo(personal);
  await expect.poll(async () => rail.getByRole("radio").evaluateAll((icons) =>
    icons.map((icon) => icon.getAttribute("aria-label")),
  )).toEqual(["All accounts, 1 unread", "demo-2@example.com", "demo@example.com, 1 unread"]);

  // Reordering also updates the sort-order-based account shortcuts.
  await page.keyboard.press("ControlOrMeta+1");
  await expect(page.getByRole("heading", { name: "0 conversations" })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+2");
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();
});

test("the account color picker keeps the last color picked, even while dragging rapidly", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Accounts", exact: true }).click();
  const swatch = settings.locator('input[aria-label="Color for demo@example.com"]');
  const initial = await swatch.inputValue();

  // `<input type="color">` fires `input` continuously while its native
  // picker is open, not just once on commit — simulate a drag through
  // several intermediate colors landing on a final one. Setting `.value`
  // through the native property setter (rather than the plain JS property,
  // which React's controlled-input tracking treats as a no-op) is required
  // for React to see each change as real, same as a genuine picker drag.
  for (const value of ["#111111", "#222222", "#333333", "#abcdef"]) {
    await swatch.evaluate((input: HTMLInputElement, value: string) => {
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")!.set!;
      setter.call(input, value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }, value);
    await page.waitForTimeout(50);
  }

  await expect(swatch).toHaveValue("#abcdef");
  // Give the debounced save well past its window to land.
  await page.waitForTimeout(500);
  await expect(swatch).toHaveValue("#abcdef");

  // Reopening the panel re-fetches from the store; the final pick, not an
  // intermediate flicker, must be what was actually saved.
  await page.keyboard.press("Escape");
  await expect(settings).not.toBeVisible();
  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  await expect(settings).toBeVisible();
  await settings.getByRole("button", { name: "Accounts", exact: true }).click();
  await expect(settings.locator('input[aria-label="Color for demo@example.com"]')).toHaveValue("#abcdef");
  expect(initial).not.toBe("#abcdef");
});

test("prompts to connect a Gmail account when none are connected", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();

  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Accounts", exact: true }).click();
  await settings.locator("li", { hasText: "demo@example.com" }).getByRole("button", { name: "Disconnect" }).click();
  await expect(settings.locator(".accounts-list li")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(settings).not.toBeVisible();

  await expect(page.getByText("Connect your Gmail account to start syncing mail.")).toBeVisible();
  await page.getByRole("button", { name: "Add account" }).click();
  await expect(settings).toBeVisible();
  await settings.getByRole("button", { name: "Add account" }).click();
  await expect(settings.locator(".accounts-list li")).toHaveCount(1);
});

test("opens Superhuman-compatible folder destinations", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  const eyebrow = page.locator(".thread-header .eyebrow");

  await page.keyboard.press("g");
  await page.keyboard.press("d");
  await expect(eyebrow).toHaveText("Drafts");
  await expect(page.getByRole("heading", { name: "0 drafts" })).toBeVisible();

  await page.keyboard.press("g");
  await page.keyboard.press("a");
  await expect(eyebrow).toHaveText("All Mail");
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();

  await page.keyboard.press("g");
  await page.keyboard.press("t");
  await expect(eyebrow).toHaveText("Trash");
  await expect(page.getByRole("heading", { name: "0 conversations" })).toBeVisible();

  await page.keyboard.press("g");
  await page.keyboard.press("i");
  await expect(page.getByRole("button", { name: "Inbox (g then i)" })).toHaveClass(/active/);
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();

  await page.keyboard.press("l");
  await expect(page.getByRole("dialog", { name: "Manage labels" })).toBeVisible();
});

test("marks an unread conversation read after the configured delay", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("threestrands.settings.autoReadDelaySeconds", "60");
  });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();

  const welcome = page.getByRole("option").filter({ hasText: "Welcome to ThreeStrands" });
  await expect(welcome.locator(".unread-dot")).toHaveClass(/visible/);

  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Reading", exact: true }).click();
  await settings.getByRole("spinbutton", { name: "Auto-read delay" }).fill("1");
  await page.keyboard.press("Escape");

  await expect(welcome.locator(".unread-dot")).not.toHaveClass(/visible/, { timeout: 3_000 });
  await expect(page.getByRole("button", { name: "Mark unread" })).toBeVisible();
});

test("loads message images according to the privacy setting", async ({ page }) => {
  await page.route("https://example.invalid/tracker.gif", (route) => route.fulfill({
    body: "GIF89a",
    contentType: "image/gif",
    headers: { "access-control-allow-origin": "*" },
  }));
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();

  const messageImage = page.frameLocator('[data-testid="message-body"]').locator("img");
  await expect(messageImage).not.toHaveAttribute("src");
  await expect(page.getByText("Images are blocked in this message.")).toBeVisible();

  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Privacy", exact: true }).click();
  await settings.getByRole("checkbox", { name: "Load remote images automatically" }).check();
  await page.keyboard.press("Escape");

  await expect(messageImage).toHaveAttribute("src", /^data:image\/gif;base64,/);
  await expect(page.getByText("Images are blocked in this message.")).not.toBeVisible();
});

test("archived and trashed threads move between Inbox, All Mail, and Trash", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();
  const roadmap = /Phase 1: read and triage/;

  await page.getByRole("option", { name: roadmap }).click();
  await page.keyboard.press("e");
  await expect(page.getByRole("heading", { name: "2 conversations" })).toBeVisible();

  // Archived, so it's gone from Inbox but still shows in All Mail.
  await page.keyboard.press("g");
  await page.keyboard.press("a");
  await expect(page.getByRole("option", { name: roadmap })).toBeVisible();

  // Trash it from All Mail; it leaves All Mail and lands in Trash.
  await page.getByRole("option", { name: roadmap }).click();
  await page.keyboard.press("#");
  await expect(page.getByRole("option", { name: roadmap })).not.toBeVisible();

  await page.keyboard.press("g");
  await page.keyboard.press("t");
  await expect(page.getByRole("option", { name: roadmap })).toBeVisible();

  // Restoring from Trash returns it to the Inbox.
  await page.getByRole("option", { name: roadmap }).click();
  await page.getByRole("button", { name: "Restore" }).click();
  await expect(page.getByRole("option", { name: roadmap })).not.toBeVisible();

  await page.keyboard.press("g");
  await page.keyboard.press("i");
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();
  await expect(page.getByRole("option", { name: roadmap })).toBeVisible();
});

test("shows folder labels and shortcuts on hover", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();

  await page.getByRole("button", { name: "Inbox (g then i)" }).hover();
  const inboxTooltip = page.getByRole("tooltip").filter({ hasText: "Inbox" });
  await expect(inboxTooltip).toBeVisible();
  await expect(inboxTooltip.locator("strong")).toHaveText("Inbox");
  await expect(inboxTooltip.locator("kbd")).toHaveText("G I");

  await page.getByRole("button", { name: /Drafts .*g then d/ }).hover();
  const draftsTooltip = page.getByRole("tooltip").filter({ hasText: "Drafts" });
  await expect(draftsTooltip).toBeVisible();
  await expect(draftsTooltip.locator("kbd")).toHaveText("G D");

  await page.getByRole("button", { name: "Labels (l)" }).hover();
  const labelsTooltip = page.getByRole("tooltip").filter({ hasText: "Manage Labels" });
  await expect(labelsTooltip).toBeVisible();
  await expect(labelsTooltip.locator("kbd")).toHaveText("L");

  await page.getByRole("button", { name: /Outbox/ }).hover();
  const outboxTooltip = page.getByRole("tooltip").filter({ hasText: "Outbox" });
  await expect(outboxTooltip).toBeVisible();
  await expect(outboxTooltip.locator("kbd")).toHaveCount(0);
});

test("switches themes and remembers the choice after reload", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/");
  await page.getByRole("button", { name: "Switch to light mode" }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator(".reader")).toHaveCSS("background-color", "rgb(255, 255, 255)");
  await page.reload();
  await expect(page.getByRole("button", { name: "Switch to dark mode" })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.getByRole("button", { name: "Switch to dark mode" }).click();
  await expect(page.locator(".reader")).toHaveCSS("background-color", "rgb(18, 18, 20)");
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
});

test("resizes the inbox with pointer and keyboard and restores the preferred width", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  const divider = page.getByRole("separator", { name: "Resize inbox" });
  await expect(divider).toHaveAttribute("aria-valuenow", "400");
  const bounds = (await divider.boundingBox())!;
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + 150);
  await page.mouse.down();
  await page.mouse.move(bounds.x + bounds.width / 2 + 120, bounds.y + 150);
  await page.mouse.up();
  await expect(page.locator(".thread-column")).toHaveCSS("width", "520px");
  await expect.poll(() => page.evaluate(() => localStorage.getItem("threestrands.inboxWidth"))).toBe("520");
  await page.reload();
  await expect(divider).toHaveAttribute("aria-valuenow", "520");
  await divider.focus();
  await page.keyboard.press("ArrowLeft");
  await expect(divider).toHaveAttribute("aria-valuenow", "510");
  await page.setViewportSize({ width: 900, height: 800 });
  await expect(divider).toHaveAttribute("aria-valuenow", "422");
  await page.setViewportSize({ width: 1280, height: 800 });
  await expect(divider).toHaveAttribute("aria-valuenow", "510");
  await page.keyboard.press("Home");
  await expect(divider).toHaveAttribute("aria-valuenow", "280");
  await divider.dblclick();
  await expect(divider).toHaveAttribute("aria-valuenow", "400");
});

test("wraps batch actions and shows their help at the minimum inbox width", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  const divider = page.getByRole("separator", { name: "Resize inbox" });
  await divider.focus();
  await page.keyboard.press("Home");
  await expect(divider).toHaveAttribute("aria-valuenow", "280");

  const selectedThread = page.getByRole("option", { selected: true });
  await selectedThread.focus();
  await page.keyboard.press("x");

  const toolbar = page.getByRole("toolbar", { name: "Batch actions" });
  await expect(toolbar).toBeVisible();
  await expect(toolbar.getByRole("button", { name: /star/i })).toHaveCount(1);
  await expect(toolbar.locator(".batch-count")).toHaveCSS("white-space", "nowrap");

  const actionLayout = await toolbar.evaluate((toolbarElement) => {
    const buttons = [...toolbarElement.querySelectorAll("button")];
    const countBounds = toolbarElement.querySelector(".batch-count")!.getBoundingClientRect();
    const firstButtonBounds = buttons[0].getBoundingClientRect();
    const toolbarBounds = toolbarElement.getBoundingClientRect();
    return {
      rows: new Set(buttons.map((button) => Math.round(button.getBoundingClientRect().top))).size,
      overflows: buttons.some((button) => {
        const buttonBounds = button.getBoundingClientRect();
        return buttonBounds.left < toolbarBounds.left || buttonBounds.right > toolbarBounds.right;
      }),
      countCenter: countBounds.top + countBounds.height / 2,
      firstRowCenter: firstButtonBounds.top + firstButtonBounds.height / 2,
    };
  });
  expect(actionLayout.rows).toBeGreaterThan(1);
  expect(actionLayout.overflows).toBe(false);
  expect(actionLayout.countCenter).toBeCloseTo(actionLayout.firstRowCenter, 0);

  await toolbar.getByRole("button", { name: "Archive" }).hover();
  await expect(toolbar.getByRole("tooltip", { name: "Archive" })).toBeVisible();
});

test("keeps the theme toggle on screen with a long email", async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 600 });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/");
  await expect(page.getByTestId("message-body")).toBeVisible();
  await page.getByTestId("message-body").evaluate((body) => {
    body.innerHTML = '<p>A long email paragraph.</p>'.repeat(150);
  });
  const toggle = page.getByRole("button", { name: "Switch to light mode" });
  await expect(toggle).toBeInViewport();
  await toggle.click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.locator(".message-stack")).toHaveJSProperty("scrollTop", 0);
});
