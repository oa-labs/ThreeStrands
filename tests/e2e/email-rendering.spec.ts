import { expect, test } from "@playwright/test";

test.describe("email rendering fixtures", () => {
  test("plain text respects the floor and keeps a larger app font scale", async ({ page }) => {
    await page.goto("/tests/email-rendering.html?theme=light&fixture=smallTextFlow&minimumFontSize=18&plain");
    const body = page.getByTestId("message-body");
    await expect(body).toHaveCSS("font-size", "18px");
    await page.evaluate(() => document.documentElement.style.setProperty("--font-scale", "1.4"));
    await expect(body).toHaveCSS("font-size", "21px");
    await page.getByRole("combobox", { name: "Minimum email font size" }).selectOption("22");
    await expect(body).toHaveCSS("font-size", "22px");
    await page.getByRole("combobox", { name: "Minimum email font size" }).selectOption("0");
    await expect(body).toHaveCSS("font-size", "21px");
  });

  for (const fixture of ["smallTextTable", "smallTextFlow"]) {
    test(`${fixture} enforces a reversible font floor across styles and responsive layout`, async ({ page }) => {
      await page.goto(`/tests/email-rendering.html?theme=light&fixture=${fixture}&minimumFontSize=18`);
      const frame = page.locator("iframe.message-body");
      const sizes = () => frame.evaluate((element) => {
        const doc = (element as HTMLIFrameElement).contentDocument!;
        return Array.from(doc.body.querySelectorAll("*"))
          .filter((node) => Array.from(node.childNodes).some((child) => child.nodeType === Node.TEXT_NODE && child.textContent?.trim()))
          .map((node) => parseFloat(doc.defaultView!.getComputedStyle(node).fontSize));
      });
      await expect.poll(async () => (await sizes()).every((size) => size >= 18)).toBe(true);
      const heading = fixture === "smallTextTable" ? "span[style*='2.8em']" : "h1";
      // The inline style is rewritten when enabled, so select by text instead.
      const headingText = fixture === "smallTextTable" ? "Large relative heading" : "Large heading";
      await expect(frame.contentFrame().getByText(headingText, { exact: true })).toHaveCSS("font-size", "28px");
      if (fixture === "smallTextTable") {
        await expect(frame.contentFrame().locator("table")).toHaveCSS("border-spacing", "4px");
        await expect(frame.contentFrame().locator("td").first()).toHaveCSS("line-height", "21.6px");
        await expect(frame.contentFrame().locator("td").last()).toHaveCSS("font-size", "0px");
      } else {
        await expect(frame.contentFrame().getByText("Hidden copy")).toBeHidden();
      }
      await page.getByRole("combobox", { name: "Minimum email font size" }).selectOption("22");
      await expect.poll(async () => (await sizes()).every((size) => size >= 22)).toBe(true);
      await expect(frame.contentFrame().getByText(headingText, { exact: true })).toHaveCSS("font-size", "28px");
      await page.setViewportSize({ width: 390, height: 900 });
      await expect.poll(async () => (await sizes()).every((size) => size >= 22)).toBe(true);
      await page.getByRole("combobox", { name: "Minimum email font size" }).selectOption("0");
      await expect.poll(async () => (await sizes()).some((size) => size < 18)).toBe(true);
      await expect(frame.contentFrame().locator(heading)).toHaveCSS("font-size", "28px");
      if (fixture === "smallTextFlow") await expect(frame.contentFrame().locator(".small-copy")).toHaveCSS("font-size", "8px");
    });
  }

  for (const theme of ["light", "dark"]) {
    for (const fixture of ["notification", "transactional"]) {
      for (const width of [390, 1200]) {
        test(`${fixture} stays contained in ${theme} mode at ${width}px`, async ({ page }) => {
          await page.setViewportSize({ width, height: 900 });
          await page.goto(`/tests/email-rendering.html?theme=${theme}&fixture=${fixture}`);
          const frame = page.locator("iframe.message-body");
          await expect(frame).toBeVisible();
          await expect.poll(() => frame.evaluate((element) => (element as HTMLIFrameElement).contentDocument?.body?.textContent?.trim().length ?? 0)).toBeGreaterThan(0);
          const expectedWidth = Math.min(width - 48, 760);
          await expect(frame).toHaveCSS("width", `${expectedWidth}px`);
          const box = await frame.boundingBox();
          expect(box?.height ?? 0).toBeGreaterThan(0);
          const overflow = await frame.evaluate((element) => {
            const root = (element as HTMLIFrameElement).contentDocument!.documentElement;
            return { scrollWidth: root.scrollWidth, clientWidth: root.clientWidth };
          });
          expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth);
          await expect(page).toHaveScreenshot(`email-${fixture}-${theme}-${width}.png`, { animations: "disabled" });
        });
      }
    }
  }

  for (const { fixture, current } of [
    { fixture: "replyAttributionWithLinkedAddress", current: "Thanks, that works." },
    { fixture: "replyAttributionInsideCitation", current: "Sounds good." },
    { fixture: "replyRuleThenHeaderBlock", current: "Thanks, will do." },
    { fixture: "replyCompleteHeaderBlockWithoutQuote", current: "Thanks" },
    { fixture: "replyAngleQuotedLines", current: "This is resolved now." },
  ]) {
    test(`${fixture} folds quoted history until the reader expands it`, async ({ page }) => {
      await page.goto(`/tests/email-rendering.html?theme=light&fixture=${fixture}`);
      const body = page.locator("iframe.message-body").contentFrame().locator("body");
      await expect(body).toContainText(current);
      await expect(body).not.toContainText("Earlier message content.");

      await page.getByRole("button", { name: "Show quoted content" }).click();
      await expect(body).toContainText("Earlier message content.");
    });
  }
});
