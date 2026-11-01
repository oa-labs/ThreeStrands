import { expect, test } from "@playwright/test";

test("connects a selected Mac address book for browsing and recipient suggestions", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("listbox").getByRole("option").first()).toBeVisible();
  // Simulate the OS boundary; browser tests never request access to real contacts.
  await page.evaluate(async () => {
    const path = "/src/data/client.ts";
    const { mailClient } = await import(path);
    const originalProfiles = mailClient.listContactProfiles.bind(mailClient);
    const originalProfile = mailClient.getContactProfile.bind(mailClient);
    const originalSuggestions = mailClient.listContactSuggestions.bind(mailClient);
    let permitted = false;
    let selected: string[] = [];
    const person = { id: "derived:mac:ada", displayName: "Ada System Contact", addresses: ["ada-system@example.net"], company: "Analytical Engines", role: "Programmer", location: null, bio: null, notes: null, links: [], photoData: null, favorite: false, birthday: null, keepInTouch: { intervalDays: null, startedAt: null, snoozedUntil: null, snoozedAt: null, lastTouchAt: null }, keepInTouchDueAt: null, sentCount: 0, receivedCount: 0, lastInteractedAt: null };
    const status = () => ({ supported: true, permission: permitted ? "authorized" : "notDetermined", connection: { sources: permitted ? [{ id: "icloud", name: "iCloud" }, { id: "google", name: "Google" }] : [], selectedSources: selected, refreshedAt: null }, contactCount: selected.includes("icloud") ? 1 : 0 });
    mailClient.systemContactsStatus = async () => status();
    mailClient.discoverSystemContacts = async () => { permitted = true; return status(); };
    mailClient.refreshSystemContacts = async (sources?: string[]) => { if (sources) selected = sources; return status(); };
    mailClient.listContactProfiles = async (query = "", limit = 500, account?: string) => {
      const profiles = await originalProfiles(query, limit, account);
      return selected.includes("icloud") && `${person.displayName} ${person.addresses}`.toLowerCase().includes(query.toLowerCase()) ? [...profiles, person] : profiles;
    };
    mailClient.getContactProfile = async (id: string) => id === person.id ? (selected.includes("icloud") ? person : null) : originalProfile(id);
    mailClient.listContactSuggestions = async (account: string, query: string, limit = 8) => {
      const suggestions = await originalSuggestions(account, query, limit);
      return selected.includes("icloud") && `${person.displayName} ${person.addresses}`.toLowerCase().includes(query.toLowerCase()) ? [...suggestions, { email: person.addresses[0], displayName: person.displayName, sentCount: 0, receivedCount: 0, lastInteractedAt: "", pinned: false }] : suggestions;
    };
  });
  await page.getByRole("button", { name: "Contacts (4)" }).click();
  const contacts = page.getByRole("region", { name: "Contacts", exact: true });
  await contacts.getByRole("button", { name: "Manage Contacts…" }).click();
  await page.getByRole("button", { name: "Mac Contacts…", exact: true }).click();
  const connection = page.getByRole("dialog", { name: "Mac Contacts", exact: true });
  await connection.getByRole("button", { name: "Allow Mac Contacts…" }).click();
  await expect(connection.getByRole("checkbox", { name: "Google" })).not.toBeChecked();
  await connection.getByRole("checkbox", { name: "iCloud" }).check();
  await connection.getByRole("button", { name: "Use selected address books" }).click();
  await expect(connection.getByRole("status")).toContainText("1 contact with email addresses connected");
  await connection.getByRole("button", { name: "Close", exact: true }).click();
  await contacts.getByRole("textbox", { name: "Search Contacts" }).fill("Ada System");
  await contacts.getByRole("button", { name: /Ada System Contact/ }).click();
  await expect(contacts.getByRole("textbox", { name: "Name", exact: true })).toHaveValue("Ada System Contact");
  await expect(contacts.getByText(/Changes saved here create an independent ThreeStrands profile/)).toBeVisible();
  await page.getByRole("button", { name: "New Message (c)" }).click();
  const composer = page.getByRole("dialog", { name: "New Message" });
  await composer.getByRole("textbox", { name: "To", exact: true }).fill("Ada System");
  await expect(composer.getByRole("option").filter({ hasText: "ada-system@example.net" })).toBeVisible();
});
