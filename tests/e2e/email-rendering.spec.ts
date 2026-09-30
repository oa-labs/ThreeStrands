import { expect, test } from "@playwright/test";

test.describe("email rendering fixtures", () => {
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
});
