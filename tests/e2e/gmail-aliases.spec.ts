import { expect, test, type Page } from "@playwright/test";

async function installAliasFixture(page: Page) {
  await page.goto("/");
  await expect(page.getByRole("listbox").getByRole("option").first()).toBeVisible();
  await page.evaluate(async () => {
    const clientPath = "/src/data/client.ts", demoPath = "/src/data/demoClient.ts", datasetPath = "/src/data/demoDataset.ts";
    const [{ mailClient }, { createDemoClient }, { defaultDemoDataset, DEMO_ACCOUNT_ID }] = await Promise.all([import(clientPath), import(demoPath), import(datasetPath)]);
    const data = defaultDemoDataset();
    data.sendingIdentities = { [DEMO_ACCOUNT_ID]: [
      { address: DEMO_ACCOUNT_ID },
      { address: "work@example.org", displayName: "Work", isDefault: true },
      { address: "support@example.org", displayName: "Support" },
    ] };
    data.messages.welcome = [{ sender: "Customer <customer@example.net>", recipients: ["Support <support@example.org>"], sentAt: "2026-10-31T10:00:00Z", bodyHtml: "<p>Support request</p>", bodyText: "Support request" }];
    Object.assign(mailClient, createDemoClient(data));
  });
}

test("Gmail defaults and selected aliases survive saving and reopening a draft", async ({ page }) => {
  await installAliasFixture(page);
  await page.keyboard.press("c");
  const composer = page.getByRole("dialog", { name: "New Message" });
  const from = composer.getByRole("combobox", { name: "Sending Address" });
  await expect(from).toHaveValue("work@example.org");
  await composer.getByRole("textbox", { name: "To", exact: true }).fill("recipient@example.net");
  await composer.getByRole("textbox", { name: "Subject" }).fill("Alias persistence");
  await from.selectOption("support@example.org");
  await expect(from).toHaveValue("support@example.org");
  await composer.getByRole("button", { name: "Save and Close Draft" }).click();
  await page.getByRole("button", { name: /Choose folder, current folder/ }).click();
  await page.getByRole("group", { name: "Folders" }).getByRole("button", { name: /Drafts/ }).click();
  await page.getByRole("option", { name: /Alias persistence/ }).click();
  await expect(from).toHaveValue("support@example.org");
});

test("a Gmail reply uses the addressed alias and leaves own aliases out of Reply All", async ({ page }) => {
  await installAliasFixture(page);
  await page.keyboard.press("a");
  const composer = page.getByRole("dialog", { name: "Reply Message" });
  await expect(composer.getByRole("combobox", { name: "Sending Address" })).toHaveValue("support@example.org");
  await expect(composer.getByRole("button", { name: "Remove Customer", exact: true })).toBeVisible();
  await expect(composer.getByRole("button", { name: /^Remove / })).toHaveCount(1);
});
