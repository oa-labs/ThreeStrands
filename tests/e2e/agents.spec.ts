import { test, expect } from "@playwright/test";

test("delegates from mail, preserves a draft across views, answers, and revises a report", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("option", { name: /Welcome to ThreeStrands/ }).click();
  await page.getByRole("button", { name: "Delegate work…", exact: true }).click();
  const form = page.getByRole("form", { name: "New delegated task" });
  await expect(form.getByText("Linked to the selected conversation.")).toBeVisible();
  await form.getByRole("button", { name: "Enable local demonstration" }).click();
  await form.getByRole("button", { name: "Start demonstration" }).click();
  // On narrow windows the conversation has a tab; this run uses both layouts.
  await page.getByRole("textbox", { name: "Your answer" }).fill("Prioritize possible customers");
  await page.getByRole("button", { name: "Inbox (1)", exact: true }).click();
  await page.getByRole("button", { name: "Agents (6)", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Your answer" })).toHaveValue("Prioritize possible customers");
  await page.getByRole("button", { name: "Answer question", exact: true }).click();
  await expect(page.getByText("Demonstration report · Not researched")).toBeVisible();
  await page.getByRole("textbox", { name: "Follow-up message" }).fill("Focus on podcast guests");
  await page.getByRole("button", { name: "Revise report", exact: true }).click();
  const versions = page.getByRole("combobox", { name: "Report version" });
  await expect(versions.locator("option")).toHaveCount(2);
  await expect(page.getByText("Revision request: Focus on podcast guests", { exact: false })).toBeVisible();
  const first = await versions.locator("option").first().getAttribute("value");
  await versions.selectOption(first!);
  await expect(page.getByText("Revision request: Focus on podcast guests", { exact: false })).toHaveCount(0);
  await page.setViewportSize({ width: 800, height: 900 });
  await page.getByRole("tab", { name: "Conversation", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "Follow-up message" })).toBeVisible();
  await page.getByRole("tab", { name: "Result", exact: true }).click();
  await expect(versions).toBeVisible();
  await expect(page.getByRole("textbox", { name: "Follow-up message" })).toBeHidden();
});
