import { expect, test, type Page } from "@playwright/test";

async function openToday(page: Page, calendar = false) {
  await page.clock.setFixedTime(new Date(2026, 10, 17, 10));
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to ThreeStrands" })).toBeVisible();
  if (calendar) {
    await page.getByRole("button", { name: /^Settings \((⌘|Ctrl\+),\)$/ }).click();
    const settings = page.getByRole("dialog", { name: "Settings" });
    await settings.getByRole("button", { name: "Calendar Accounts", exact: true }).click();
    await settings.getByRole("button", { name: "Connect Calendar", exact: true }).click();
    await expect(settings.getByRole("button", { name: "Connect Calendar", exact: true })).toBeEnabled();
    await settings.getByRole("button", { name: "Close", exact: true }).click();
  }
  await page.getByRole("button", { name: "Today (0)", exact: true }).click();
  return page.getByRole("region", { name: "Today", exact: true });
}

test("Today's columns fill the available height and clip the grid to the rounded schedule border", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const today = await openToday(page, true);
  await expect(today.getByRole("group", { name: "Day view" })).toBeVisible();
  for (const height of [900, 1200]) {
    await page.setViewportSize({ width: 1440, height });
    await expect.poll(() => today.evaluate((root) => {
      const box = (selector: string) => root.querySelector<HTMLElement>(selector)!;
      // The left column (Planning Inbox above the Done card) ends level with the schedule.
      const inbox = box(".today-stack > .today-work:last-child").getBoundingClientRect();
      const schedule = box(".today-time").getBoundingClientRect();
      const grid = box(".calendar-week-scroll").getBoundingClientRect();
      const columns = box(".today-columns");
      const gutter = parseFloat(getComputedStyle(columns).paddingBottom);
      const pane = root.getBoundingClientRect();
      return { scheduleGap: Math.round(schedule.bottom + gutter - pane.bottom), columnGap: Math.round(inbox.bottom - schedule.bottom), gridGap: Math.round(grid.bottom - (schedule.bottom - 1)) };
    })).toEqual({ scheduleGap: 0, columnGap: 0, gridGap: 0 });
  }
  const schedule = today.locator(".today-time");
  await expect(schedule).toHaveCSS("overflow", "hidden");
  expect(parseFloat(await schedule.evaluate((element) => getComputedStyle(element).borderBottomLeftRadius))).toBeGreaterThan(0);
  expect(parseFloat(await schedule.evaluate((element) => getComputedStyle(element).borderBottomRightRadius))).toBeGreaterThan(0);
});

test("completes a starred suggestion through the task form, preserves the email, and reopens the work", async ({ page }) => {
  const today = await openToday(page);
  const inbox = today.getByRole("region", { name: "Planning Inbox", exact: true });
  await inbox.getByRole("tab", { name: "Emails", exact: true }).click();
  const checkbox = inbox.getByRole("checkbox", { name: /^Mark .* Done$/ }).first();
  const label = await checkbox.getAttribute("aria-label");
  await checkbox.click();
  let form = page.getByRole("dialog", { name: "Add Task", exact: true });
  await expect(form).toBeVisible();
  await form.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(today.getByRole("checkbox", { name: label! })).not.toBeChecked();
  await today.getByRole("checkbox", { name: label! }).click();
  form = page.getByRole("dialog", { name: "Add Task", exact: true });
  await form.getByRole("textbox", { name: "Task", exact: true }).fill("Review the roadmap");
  await form.getByRole("button", { name: "Add Task", exact: true }).click();
  const completed = today.getByRole("region", { name: "Done Today", exact: true });
  await expect(completed.getByRole("checkbox", { name: "Mark Review the roadmap Not Done" })).toBeChecked();
  const saved = await page.evaluate(async () => {
    const path = "/src/data/client.ts";
    const { mailClient } = await import(path);
    return { thread: (await mailClient.getThread("roadmap")).thread, tasks: await mailClient.listTasks() };
  });
  expect(saved.thread.starred).toBe(true);
  expect(saved.thread.archived).toBe(false);
  expect(saved.tasks.find((task: { title: string }) => task.title === "Review the roadmap")).toMatchObject({ status: "completed", sourceMessageId: expect.any(String) });
  await page.keyboard.press("1");
  await expect(today).toBeHidden();
  await page.getByRole("button", { name: "Today (0)", exact: true }).click();
  await expect(completed.getByRole("checkbox", { name: "Mark Review the roadmap Not Done" })).toBeChecked();
  await completed.getByRole("checkbox", { name: "Mark Review the roadmap Not Done" }).click();
  await inbox.getByRole("tab", { name: "Tasks", exact: true }).click();
  await expect(inbox.getByRole("checkbox", { name: "Mark Review the roadmap Done" })).not.toBeChecked();
});
