import { expect, test } from "@playwright/test";

const threadOptions = (page: import("@playwright/test").Page) => page.getByRole("listbox").getByRole("option");

test("keeps a long suggested task inside the right panel", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator(".context-panel")).toBeVisible();

  const layout = await page.evaluate(() => {
    const panel = document.querySelector<HTMLElement>(".context-panel");
    if (!panel) throw new Error("Missing context panel");
    const section = document.createElement("section");
    section.className = "context-section thread-assist";
    section.innerHTML = `<div class="thread-assist-suggestions"><h4>Suggested</h4><div class="action-proposals"><article class="action-proposal-card"><strong>Review shared Google Doc and reply with thoughts</strong><p>Link from Mark: https://docs.google.com/document/d/${"A".repeat(180)}/edit?usp=sharing</p></article></div></div>`;
    panel.append(section);
    const card = section.querySelector<HTMLElement>(".action-proposal-card")!;
    const panelRight = panel.getBoundingClientRect().right - parseFloat(getComputedStyle(panel).paddingRight);
    return { cardRight: card.getBoundingClientRect().right, panelRight, cardScrollWidth: card.scrollWidth, cardClientWidth: card.clientWidth };
  });

  expect(layout.cardRight).toBeLessThanOrEqual(layout.panelRight + 1);
  expect(layout.cardScrollWidth).toBeLessThanOrEqual(layout.cardClientWidth + 1);
});

test("gives the conversation shortcut key breathing room", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator(".context-panel")).toBeVisible();

  const padding = await page.evaluate(() => {
    const prompt = document.createElement("button");
    prompt.className = "thread-chat-prompt";
    prompt.innerHTML = "<span>Ask about this conversation…</span><kbd>q</kbd>";
    document.querySelector(".context-panel")!.append(prompt);
    const style = getComputedStyle(prompt.querySelector("kbd")!);
    return { top: parseFloat(style.paddingTop), right: parseFloat(style.paddingRight), bottom: parseFloat(style.paddingBottom), left: parseFloat(style.paddingLeft) };
  });

  expect(padding.top).toBeGreaterThanOrEqual(2);
  expect(padding.bottom).toBeGreaterThan(padding.top);
  expect(padding.left).toBeGreaterThanOrEqual(6);
  expect(padding.right).toBeGreaterThanOrEqual(6);
});

test("keeps meeting titles and recent email subjects on one line in the context panel", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator(".context-panel")).toBeVisible();

  const layout = await page.evaluate(() => {
    const panel = document.querySelector(".context-panel")!;
    // Markup as ContextSection and ContextRow render it: an icon row, a row with no glyph, and a row with a control.
    const header = (title: string) => `<header class="context-section-header"><h3><button class="context-section-toggle"><span class="context-row-glyph"><svg width="12" height="12"></svg></span><span>${title}</span></button></h3></header>`;
    const row = ({ icon = "", control = "", title, date, detail, extra = "" }: { icon?: string; control?: string; title: string; date: string; detail: string; extra?: string }) =>
      `<div class="context-row context-row-interactive${control ? " context-row-has-control" : ""} ${extra}">`
      + (control ? `<span class="context-row-glyph context-row-control">${control}</span>` : "")
      + `<button class="context-row-main">${control ? "" : `<span class="context-row-glyph context-row-icon">${icon}</span>`}`
      + `<span class="context-row-text"><span class="context-row-line"><strong class="context-row-title">${title}</strong><small class="context-row-date">${date}</small></span><small class="context-row-detail">${detail}</small></span></button></div>`;
    const meetings = document.createElement("section");
    meetings.className = "context-section context-collapsible context-meetings";
    meetings.innerHTML = header("Upcoming meetings") + "<div>" + row({ icon: '<svg width="14" height="14"></svg>', title: "Momentum Prep Call - EO Pittsburgh - Week 7 and 8 ".repeat(3), date: "Mon, Oct 5", detail: `<span class="context-meeting-details">with ${"Beth Goldstein ".repeat(8)}</span><span class="context-meeting-response"> · Going</span>`, extra: "context-meeting" }) + "</div>";
    const history = document.createElement("section");
    history.className = "context-section context-collapsible context-history";
    history.innerHTML = header("Recent emails") + '<div><p class="context-section-note"><span>Sep 28 – Oct 7</span></p>' + row({ title: "Invitation: Call with Joel and McKenzie on Tuesday ".repeat(3), date: "Sep 21", detail: "beth@example.com" }) + '<button class="btn-link context-link-button">Show 2 more</button></div>';
    const tasks = document.createElement("section");
    tasks.className = "context-section context-collapsible context-tasks";
    tasks.innerHTML = header("Tasks") + "<div>" + row({ control: '<input type="checkbox" class="context-task-checkbox">', title: "Email the sync errors", date: "Due Tomorrow", detail: "Re: Data quality" }) + "</div>";
    panel.append(meetings, history, tasks);
    const left = (element: Element) => element.getBoundingClientRect().left;
    const titles = [meetings, history].map((section) => {
      const title = section.querySelector(".context-row-title")!;
      const style = getComputedStyle(title);
      const meta = getComputedStyle(section.querySelector(".context-row-detail")!);
      const main = getComputedStyle(title.closest("button")!);
      return {
        whiteSpace: style.whiteSpace, textOverflow: style.textOverflow, scrollWidth: title.scrollWidth, clientWidth: title.clientWidth,
        font: `${style.fontFamily}|${style.fontSize}|${style.fontWeight}|${style.color}`,
        metaFont: `${meta.fontFamily}|${meta.fontSize}|${meta.color}`,
        rowBackground: main.backgroundColor, rowTextAlign: main.textAlign,
      };
    });
    const dates = [meetings, history, tasks].map((section) => {
      const rowBox = section.querySelector(".context-row")!.getBoundingClientRect();
      const title = section.querySelector(".context-row-title")!.getBoundingClientRect();
      const date = section.querySelector(".context-row-date")!.getBoundingClientRect();
      const rowStyle = getComputedStyle(section.querySelector(".context-row")!);
      return { titleTop: title.top, titleBottom: title.bottom, dateTop: date.top, dateBottom: date.bottom, dateRight: date.right, rowContentRight: rowBox.right - parseFloat(rowStyle.paddingRight) };
    });
    const edges = {
      labels: [meetings, history, tasks].map((section) => left(section.querySelector(".context-section-toggle > span:last-child")!)),
      rows: [meetings, history, tasks].map((section) => left(section.querySelector(".context-row-title")!)),
      note: left(history.querySelector(".context-section-note")!),
      link: left(history.querySelector(".context-link-button")!),
    };
    const meta = meetings.querySelector<HTMLElement>(".context-row-detail")!;
    const response = meetings.querySelector<HTMLElement>(".context-meeting-response")!;
    return { titles, dates, edges, metaWhiteSpace: getComputedStyle(meta).whiteSpace, metaHeight: meta.clientHeight, responseHeight: response.clientHeight, responseRight: response.getBoundingClientRect().right, metaRight: meta.getBoundingClientRect().right };
  });

  for (const title of layout.titles) {
    expect(title.whiteSpace).toBe("nowrap");
    expect(title.textOverflow).toBe("ellipsis");
    expect(title.scrollWidth).toBeGreaterThan(title.clientWidth);
    expect(title.rowBackground).toBe("rgba(0, 0, 0, 0)");
    expect(title.rowTextAlign).toBe("left");
  }
  // Recent email rows use the same type as meeting rows, not browser button defaults.
  expect(layout.titles[1].font).toBe(layout.titles[0].font);
  expect(layout.titles[1].metaFont).toBe(layout.titles[0].metaFont);
  expect(layout.metaWhiteSpace).toBe("nowrap");
  expect(layout.metaHeight).toBeLessThanOrEqual(layout.responseHeight + 1);
  expect(layout.responseRight).toBeLessThanOrEqual(layout.metaRight + 1);

  // One text edge: section labels, row titles (with an icon, no glyph, or a checkbox), notes, and links.
  const edge = layout.edges.labels[0];
  for (const value of [...layout.edges.labels, ...layout.edges.rows, layout.edges.note, layout.edges.link]) expect(Math.abs(value - edge)).toBeLessThanOrEqual(0.5);
  // The date always ends the title line, at the row's right edge.
  for (const date of layout.dates) {
    expect(date.dateTop).toBeGreaterThanOrEqual(date.titleTop - 1);
    expect(date.dateBottom).toBeLessThanOrEqual(date.titleBottom + 1);
    expect(Math.abs(date.dateRight - date.rowContentRight)).toBeLessThanOrEqual(0.5);
  }
});

test("keeps the sender contact card to three type sizes with details below the email address", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  // The context panel no longer has a contact card; the reader shows it on hover.
  await expect(page.locator(".context-panel .context-contact-text")).toHaveCount(0);
  await page.locator(".message-sender-row .address").first().hover();
  const text = page.locator(".address-card .context-contact-text").first();
  await expect(text).toBeVisible();

  const sizes = await text.evaluate((element) => {
    element.insertAdjacentHTML("beforeend", '<p>Managing Director · Acme</p><p class="context-contact-activity">10 emails since Jun 2026 · You last wrote Sep 29</p><p>Phoenix, AZ</p>');
    const size = (node: Element) => parseFloat(getComputedStyle(node).fontSize);
    const activity = element.querySelector<HTMLElement>("p.context-contact-activity")!;
    return {
      name: size(element.querySelector(".context-contact-title")!),
      email: size(element.querySelector(".contact-sidebar-email-row a span")!),
      details: [...element.querySelectorAll("p")].map(size),
      activityLines: Math.round(activity.getBoundingClientRect().height / parseFloat(getComputedStyle(activity).lineHeight)),
    };
  });

  expect(new Set([sizes.name, sizes.email, ...sizes.details]).size).toBeLessThanOrEqual(3);
  expect(new Set(sizes.details).size).toBe(1);
  expect(sizes.details[0]).toBeLessThan(sizes.email);
  expect(sizes.email).toBeLessThan(sizes.name);
  // The card widens for a typical history line instead of wrapping it.
  expect(sizes.activityLines).toBe(1);
});

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
  await expect(page.getByRole("listbox").getByRole("option", { selected: true })).toContainText("Your inbox stays local");
  await expect(page.getByRole("heading", { name: "Your inbox stays local" })).toBeVisible();

  await expect(page.getByRole("status")).toBeHidden({ timeout: 10_000 });
});

test("keeps a sender contact card open while moving to its copy button", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: async () => undefined },
    });
  });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();

  const address = page.locator(".message-sender-row .address").first();
  // The card renders at the top of the page, so find it there rather than inside the address.
  const popover = page.locator(".address-card");
  // Other people's addresses open the contact card, whose copy button names the action.
  const copy = popover.getByRole("button", { name: "Copy email address" });

  await address.hover();
  await expect(popover).toBeVisible();
  await expect(popover).toContainText("hello@threestrands.local");
  // Nothing covers or clips the card: its far corner is the card itself and it fits in the window.
  const onTop = await popover.evaluate((card) => {
    const box = card.getBoundingClientRect();
    const hit = document.elementFromPoint(box.right - 4, box.bottom - 4);
    return { covered: !card.contains(hit), right: box.right, width: window.innerWidth };
  });
  expect(onTop.covered).toBe(false);
  expect(onTop.right).toBeLessThanOrEqual(onTop.width);

  const addressBox = await address.boundingBox();
  const popoverBox = await popover.boundingBox();
  expect(addressBox).not.toBeNull();
  expect(popoverBox).not.toBeNull();
  if (!addressBox || !popoverBox) return;

  await page.mouse.move(popoverBox.x + 4, (addressBox.y + addressBox.height + popoverBox.y) / 2);
  await expect(popover).toBeVisible();
  await copy.hover();
  await copy.click();
  await expect(popover.getByRole("button", { name: "Copied email address" })).toBeVisible();
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

  await dialog.getByRole("button", { name: "Send One-Click Request" }).click();
  await expect(page.getByRole("status")).toContainText("Unsubscribe request sent");
  await expect(dialog).not.toBeVisible();
});

test("searches and opens the command palette", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("textbox", { name: "Search Mail" })).toHaveCount(0);
  await expect(page.locator(".thread-header").getByRole("button", { name: "Filters" })).toBeVisible();
  await expect(page.locator(".sidebar").getByRole("button", { name: "Refresh Mail" })).toBeVisible();
  await page.keyboard.press("/");
  const search = page.getByRole("textbox", { name: "Search Mail" });
  await expect(search).toBeFocused();
  await search.fill("SQLite");
  await expect(threadOptions(page)).toHaveCount(1);
  await expect(page.getByRole("heading", { name: "Your inbox stays local" })).toBeVisible();

  await page.keyboard.press("ControlOrMeta+k");
  const palette = page.getByRole("dialog", { name: "Command Palette" });
  await expect(palette).toBeVisible();
  await expect(palette.getByRole("button", { name: /Increase Font Size/ })).toContainText("⌘/Ctrl + =");
  await expect(palette.getByRole("button", { name: /Increase Font Size/ })).toContainText("⌘/Ctrl + +");
  await expect(palette.getByRole("button", { name: /Decrease Font Size/ })).toContainText("⌘/Ctrl + -");
});

test("dismisses search with Escape", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  await page.keyboard.press("/");
  const search = page.getByRole("textbox", { name: "Search Mail" });
  await search.fill("SQLite");
  await expect(threadOptions(page)).toHaveCount(1);

  await page.keyboard.press("Escape");
  await expect(search).toHaveCount(0);
  await expect(threadOptions(page)).toHaveCount(3);
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
  const help = page.getByRole("dialog", { name: "Keyboard Shortcuts" });
  await expect(help).toBeVisible();
  await expect(help.getByRole("heading", { name: "Navigation" })).toBeVisible();
  await expect(help).toContainText("Go to Inbox");
  await expect(help).toContainText("Manage Labels");
  await expect(help).toContainText("New Message");
  await expect(help).toContainText("Command Palette");
  await expect(help).toContainText("Undo Last Action");

  await page.keyboard.press("Escape");
  await expect(help).not.toBeVisible();

  await page.keyboard.press("/");
  const search = page.getByRole("textbox", { name: "Search Mail" });
  await search.focus();
  await page.keyboard.type("?");
  await expect(search).toHaveValue("?");
  await expect(help).not.toBeVisible();

  // The sidebar has no dedicated button; the palette entry is the discoverable path.
  await expect(page.getByRole("button", { name: /Keyboard Shortcuts/ })).toHaveCount(0);
  await page.getByRole("button", { name: "Command Palette" }).click();
  const palette = page.getByRole("dialog", { name: "Command Palette" });
  await palette.getByRole("textbox", { name: "Filter Commands" }).fill("keyboard");
  await palette.getByRole("button", { name: /Keyboard Shortcuts/ }).click();
  await expect(palette).not.toBeVisible();
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
  await settings.getByRole("button", { name: "Mail Accounts", exact: true }).click();
  await settings.getByRole("button", { name: "Add Account" }).click();
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
  let palette = page.getByRole("dialog", { name: "Command Palette" });
  await palette.getByRole("button", { name: /Switch to demo-2@example.com/ }).click();
  await expect(page.getByRole("heading", { name: "0 conversations" })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+k");
  palette = page.getByRole("dialog", { name: "Command Palette" });
  await palette.getByRole("button", { name: /Show All Accounts/ }).click();
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();

  // A new message offers a From selector once more than one account is connected.
  await page.keyboard.press("c");
  const composer = page.getByRole("dialog", { name: "New Message" });
  const from = composer.getByRole("combobox", { name: "Send From" });
  const subject = composer.getByRole("textbox", { name: "Subject" });
  await expect(from).toHaveValue("demo@example.com");
  // From and Subject share the app-wide control height (--control-h), the same as buttons.
  await expect(from).toHaveCSS("box-sizing", "border-box");
  await expect(from).toHaveCSS("height", "34px");
  await expect(subject).toHaveCSS("box-sizing", "border-box");
  await expect(subject).toHaveCSS("height", "34px");
  await from.selectOption("demo-2@example.com");
  await expect(from).toHaveValue("demo-2@example.com");
  await composer.getByRole("button", { name: "Discard Draft" }).click();
  await expect(composer).not.toBeVisible();

  // Shortcut help documents the new per-account bindings.
  await page.keyboard.press("Shift+/");
  const help = page.getByRole("dialog", { name: "Keyboard Shortcuts" });
  await expect(help).toContainText("Switch to demo@example.com");
  await expect(help).toContainText("Switch to demo-2@example.com");
  await expect(help).toContainText("Show All Accounts");
  await page.keyboard.press("Escape");
  await expect(help).not.toBeVisible();

  // The sidebar rail lists both accounts and can switch back to "All accounts", still showing per-thread account dots.
  await expect(rail.getByRole("radio", { name: /demo-2@example.com/ })).toBeVisible();
  await rail.getByRole("radio", { name: "All Accounts" }).click();
  await expect(page.locator(".thread-row .account-dot").first()).toBeVisible();

  // Removing the second account keeps the first account identifiable and its inbox unaffected.
  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  await expect(settings).toBeVisible();
  await settings.getByRole("button", { name: "Mail Accounts", exact: true }).click();
  const cardSecond = settings.locator("li", { hasText: "demo-2@example.com" });
  await cardSecond.getByRole("button", { name: "Disconnect…" }).click();
  await cardSecond.getByRole("button", { name: "Disconnect this device" }).click();
  await expect(settings.locator(".accounts-list li")).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(settings).not.toBeVisible();
  await expect(rail.getByRole("radio")).toHaveCount(1);
  await rail.getByRole("radio", { name: /demo@example.com/ }).hover();
  await expect(page.getByRole("tooltip", { name: "demo@example.com" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();
  await page.keyboard.press("j");
  await expect(page.getByRole("heading", { name: "Phase 1: read and triage" })).toBeVisible();
});

test("saves an independent sender name for each account", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Mail Accounts", exact: true }).click();
  await settings.getByRole("button", { name: "Add Account" }).click();

  const personalName = settings.getByRole("textbox", { name: "Sender name for demo@example.com" });
  await personalName.fill("Joel Reed");
  await settings.locator(".account-card").filter({ hasText: "demo@example.com" }).getByRole("button", { name: "Save Name" }).click();

  const workName = settings.getByRole("textbox", { name: "Sender name for demo-2@example.com" });
  await workName.fill("Joel at Work");
  await settings.locator(".account-card").filter({ hasText: "demo-2@example.com" }).getByRole("button", { name: "Save Name" }).click();

  await expect(personalName).toHaveValue("Joel Reed");
  await expect(workName).toHaveValue("Joel at Work");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  await settings.getByRole("button", { name: "Mail Accounts", exact: true }).click();
  await expect(settings.getByRole("textbox", { name: "Sender name for demo@example.com" })).toHaveValue("Joel Reed");
  await expect(settings.getByRole("textbox", { name: "Sender name for demo-2@example.com" })).toHaveValue("Joel at Work");
});

test("reorders navbar accounts by dragging their icons", async ({ page }) => {
  await page.goto("/");

  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Mail Accounts", exact: true }).click();
  await settings.getByRole("button", { name: "Add Account" }).click();
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
  await settings.getByRole("button", { name: "Mail Accounts", exact: true }).click();
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
  await settings.getByRole("button", { name: "Mail Accounts", exact: true }).click();
  await expect(settings.locator('input[aria-label="Color for demo@example.com"]')).toHaveValue("#abcdef");
  expect(initial).not.toBe("#abcdef");
});

test("prompts to connect a Gmail account when none are connected", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();

  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Mail Accounts", exact: true }).click();
  const cardOnly = settings.locator("li", { hasText: "demo@example.com" });
  await cardOnly.getByRole("button", { name: "Disconnect…" }).click();
  await cardOnly.getByRole("button", { name: "Disconnect this device" }).click();
  await expect(settings.locator(".accounts-list li")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(settings).not.toBeVisible();

  await expect(page.getByText("Connect your Gmail account to start syncing mail.")).toBeVisible();
  await page.getByRole("button", { name: "Add Account" }).click();
  await expect(settings).toBeVisible();
  await settings.getByRole("button", { name: "Add Account" }).click();
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
  await page.keyboard.press("o");
  await expect(eyebrow).toHaveText("Outbox");
  await expect(page.getByRole("heading", { name: "0 outgoing" })).toBeVisible();

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
  await expect(page.getByRole("button", { name: "Choose folder, current folder Inbox" })).toBeVisible();
  await expect(page.getByRole("tab", { name: /^Main/ })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();

  await page.keyboard.press("l");
  await expect(page.getByRole("dialog", { name: "Manage Labels" })).toBeVisible();
});

test("marks an unread conversation read after the configured delay", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("threestrands.settings.autoReadDelaySeconds", "60");
  });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();

  const welcome = threadOptions(page).filter({ hasText: "Welcome to ThreeStrands" });
  await expect(welcome.locator(".unread-dot")).toHaveClass(/visible/);

  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Appearance", exact: true }).click();
  await settings.getByRole("spinbutton", { name: "Auto-Read Delay" }).fill("1");
  await page.keyboard.press("Escape");

  await expect(welcome.locator(".unread-dot")).not.toHaveClass(/visible/, { timeout: 3_000 });
  await expect(page.getByRole("button", { name: "Mark Unread" })).toBeVisible();
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
  await settings.getByRole("checkbox", { name: "Load Remote Images Automatically" }).check();
  await page.keyboard.press("Escape");

  await expect(messageImage).toHaveAttribute("src", /^data:image\/gif;base64,/);
  await expect(page.getByText("Images are blocked in this message.")).not.toBeVisible();
});

test("keeps sync diagnostics and crash reports together in Settings", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();

  await page.getByRole("button", { name: "Settings (⌘,)" }).click();
  const settings = page.getByRole("dialog", { name: "Settings" });
  await settings.getByRole("button", { name: "Diagnostics", exact: true }).click();

  await expect(settings.getByRole("heading", { name: "Sync Health" })).toBeVisible();
  await expect(settings.getByText("Sync is healthy")).toBeVisible();
  await expect(settings.getByRole("heading", { name: "Crash Reports" })).toBeVisible();
  await expect(settings.getByRole("checkbox", { name: "Share Sanitized Crash Reports" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Sync diagnostics need attention" })).not.toBeVisible();
});

test("archived and trashed threads move between Inbox, All Mail, and Trash", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();
  const roadmap = /Phase 1: read and triage/;

  await page.getByRole("listbox").getByRole("option", { name: roadmap }).click();
  await page.keyboard.press("e");
  await expect(page.getByRole("heading", { name: "2 conversations" })).toBeVisible();

  // Archived, so it's gone from Inbox but still shows in All Mail.
  await page.keyboard.press("g");
  await page.keyboard.press("a");
  await expect(page.getByRole("listbox").getByRole("option", { name: roadmap })).toBeVisible();

  // Trash it from All Mail; it leaves All Mail and lands in Trash.
  await page.getByRole("listbox").getByRole("option", { name: roadmap }).click();
  await page.keyboard.press("#");
  await expect(page.getByRole("listbox").getByRole("option", { name: roadmap })).not.toBeVisible();

  await page.keyboard.press("g");
  await page.keyboard.press("t");
  await expect(page.getByRole("listbox").getByRole("option", { name: roadmap })).toBeVisible();

  // Restoring from Trash returns it to the Inbox.
  await page.getByRole("listbox").getByRole("option", { name: roadmap }).click();
  await page.getByRole("button", { name: "Restore" }).click();
  await expect(page.getByRole("listbox").getByRole("option", { name: roadmap })).not.toBeVisible();

  await page.keyboard.press("g");
  await page.keyboard.press("i");
  await expect(page.getByRole("heading", { name: "3 conversations" })).toBeVisible();
  await expect(page.getByRole("listbox").getByRole("option", { name: roadmap })).toBeVisible();
});

test("shows folder labels and shortcuts in the header menu", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();

  const trigger = page.getByRole("button", { name: "Choose folder, current folder Inbox" });
  await trigger.click();
  const folders = page.getByRole("group", { name: "Folders" });
  await expect(folders.getByRole("button", { name: "Inbox" }).locator("kbd")).toHaveText("G I");
  await expect(folders.getByRole("button", { name: "Drafts" }).locator("kbd")).toHaveText("G D");
  await expect(folders.getByRole("button", { name: "Outbox" }).locator("kbd")).toHaveText("G O");
  await trigger.click();

  await page.getByRole("button", { name: "Labels (l)" }).hover();
  const labelsTooltip = page.getByRole("tooltip").filter({ hasText: "Manage Labels" });
  await expect(labelsTooltip).toBeVisible();
  await expect(labelsTooltip.locator("kbd")).toHaveText("L");

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
  const divider = page.getByRole("separator", { name: "Resize Inbox" });
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

test("lays batch actions out in two even right-aligned rows and shows their help at the minimum inbox width", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  const divider = page.getByRole("separator", { name: "Resize Inbox" });
  await divider.focus();
  await page.keyboard.press("Home");
  await expect(divider).toHaveAttribute("aria-valuenow", "280");

  const selectedThread = page.getByRole("listbox").getByRole("option", { selected: true });
  await selectedThread.focus();
  await page.keyboard.press("x");

  const toolbar = page.getByRole("toolbar", { name: "Batch actions" });
  await expect(toolbar).toBeVisible();
  await expect(toolbar.getByRole("button", { name: /star/i })).toHaveCount(1);
  await expect(toolbar.locator(".batch-count")).toHaveCSS("white-space", "nowrap");

  const actionLayout = await toolbar.evaluate((toolbarElement) => {
    const buttons = [...toolbarElement.querySelectorAll(".batch-actions button")];
    const checkboxBounds = toolbarElement.querySelector(".select-all input")!.getBoundingClientRect();
    const countBounds = toolbarElement.querySelector(".batch-count")!.getBoundingClientRect();
    const actionsBounds = toolbarElement.querySelector(".batch-actions")!.getBoundingClientRect();
    const toolbarBounds = toolbarElement.getBoundingClientRect();
    const rows = new Map<number, DOMRect[]>();
    for (const button of buttons) {
      const bounds = button.getBoundingClientRect();
      const top = Math.round(bounds.top);
      rows.set(top, [...(rows.get(top) ?? []), bounds]);
    }
    return {
      rowSizes: [...rows.values()].map((row) => row.length),
      rowRights: [...rows.values()].map((row) => Math.round(Math.max(...row.map((bounds) => bounds.right)))),
      overflows: buttons.some((button) => {
        const buttonBounds = button.getBoundingClientRect();
        return buttonBounds.left < toolbarBounds.left || buttonBounds.right > toolbarBounds.right;
      }),
      toolbarRight: Math.round(toolbarBounds.right),
      checkboxCenter: checkboxBounds.top + checkboxBounds.height / 2,
      countCenter: countBounds.top + countBounds.height / 2,
      actionsCenter: actionsBounds.top + actionsBounds.height / 2,
    };
  });
  expect(actionLayout.rowSizes).toEqual([4, 4]);
  expect(actionLayout.rowRights[0]).toBe(actionLayout.rowRights[1]);
  expect(actionLayout.rowRights[0]).toBe(actionLayout.toolbarRight);
  expect(actionLayout.overflows).toBe(false);
  expect(actionLayout.countCenter).toBeCloseTo(actionLayout.actionsCenter, 0);
  expect(actionLayout.checkboxCenter).toBeCloseTo(actionLayout.actionsCenter, 0);

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
