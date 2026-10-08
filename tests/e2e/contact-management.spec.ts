import { expect, test } from "@playwright/test";

test("merges saved people and suppresses suggestions while allowing an explicit recipient", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Contacts (4)" }).click();
  const contacts = page.getByRole("region", { name: "Contacts", exact: true });
  for (const [name, email] of [["Primary Person", "primary@example.com"], ["Other Person", "other@example.com"]]) {
    await contacts.getByRole("button", { name: "New Contact", exact: true }).click();
    await contacts.getByRole("textbox", { name: "Name", exact: true }).fill(name);
    const address = contacts.getByRole("textbox", { name: "Email addresses", exact: true });
    await address.fill(email); await address.press("Enter");
    await contacts.getByRole("button", { name: "Save contact", exact: true }).click();
    await expect(contacts.getByRole("region", { name: "Save changes" })).not.toBeVisible();
  }
  await contacts.getByRole("button", { name: "Select", exact: true }).click();
  await contacts.getByRole("checkbox", { name: "Select Primary Person" }).check();
  await contacts.getByRole("checkbox", { name: "Select Other Person" }).check();
  await contacts.getByRole("button", { name: "Merge…" }).click();
  const merge = page.getByRole("dialog", { name: "Merge contacts" });
  await merge.getByLabel("Profile to keep").selectOption({ label: "Primary Person — primary@example.com" });
  await merge.getByRole("button", { name: "Merge 2 contacts" }).click();
  await expect(merge).not.toBeVisible();
  await expect(contacts.getByRole("textbox", { name: "Name", exact: true })).toHaveValue("Primary Person");
  await expect(contacts.getByRole("button", { name: "Copy other@example.com", exact: true })).toBeVisible();
  await expect(contacts.getByRole("button", { name: /^Other Person/ })).toHaveCount(0);

  await contacts.getByRole("button", { name: "Manage Contacts…" }).click();
  await page.getByRole("button", { name: "Manage recipient suggestions…" }).click();
  const suggestions = page.getByRole("dialog", { name: "Never suggest these addresses" });
  await suggestions.getByRole("button", { name: "Never suggest other@example.com" }).click();
  await expect(suggestions.getByRole("button", { name: "Allow suggestions for other@example.com" })).toBeVisible();
  await suggestions.getByRole("button", { name: "Close", exact: true }).click();
  await page.getByRole("button", { name: "Contacts (4)" }).click();
  await page.getByRole("button", { name: "New Message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New Message" });
  const to = composer.getByRole("textbox", { name: "To", exact: true });
  // A surviving address remains suggested, exercising the merged profile.
  await to.fill("primary");
  await expect(composer.getByRole("option").filter({ hasText: "primary@example.com" })).toBeVisible();
  await to.fill("other");
  await expect(composer.getByRole("option").filter({ hasText: "other@example.com" })).not.toBeVisible();
  await to.fill("other@example.com");
  // The existing explicit “Pin … as a contact” action can remain available.
  await to.press("Escape");
  await to.press("Enter");
  await expect(composer.getByRole("button", { name: "Remove other@example.com" })).toBeVisible();
});
