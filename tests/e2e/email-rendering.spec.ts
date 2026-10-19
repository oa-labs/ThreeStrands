import { expect, test } from "@playwright/test";
import { emailRenderingFixtures } from "../../src/test/emailRenderingFixtures";

test.describe("email rendering fixtures", () => {
  for (const fixture of ["notification", "transactional"]) {
    test(`${fixture} changes read styling without reloading or losing selection`, async ({ page }) => {
      await page.goto(`/tests/email-rendering.html?theme=light&fixture=${fixture}&tone`);
      const frame = page.locator("iframe.message-body");
      const body = frame.contentFrame().locator("body");
      await expect(body).toHaveAttribute("data-tone", "default");
      const originalDocument = await frame.evaluateHandle((element) => (element as HTMLIFrameElement).contentDocument!);
      const originalSrcdoc = await frame.getAttribute("srcdoc");
      const selectedText = await frame.evaluate((element) => {
        const doc = (element as HTMLIFrameElement).contentDocument!;
        const range = doc.createRange();
        range.selectNodeContents(doc.querySelector("[data-email-root]")!);
        doc.getSelection()!.addRange(range);
        return doc.getSelection()!.toString();
      });
      expect(selectedText.length).toBeGreaterThan(0);

      for (const { tone, color } of [
        { tone: "muted", color: "rgb(98, 98, 115)" },
        { tone: "current", color: "rgb(36, 36, 44)" },
        { tone: "default", color: "rgb(53, 53, 65)" },
      ]) {
        await page.getByRole("combobox", { name: "Message tone" }).selectOption(tone);
        await expect(body).toHaveAttribute("data-tone", tone);
        await expect(body).toHaveCSS("color", color);
        expect(await frame.getAttribute("srcdoc")).toBe(originalSrcdoc);
        expect(await frame.evaluate((element, original) => (element as HTMLIFrameElement).contentDocument === original, originalDocument)).toBe(true);
        expect(await frame.evaluate((element) => (element as HTMLIFrameElement).contentDocument!.getSelection()!.toString())).toBe(selectedText);
      }
      await originalDocument.dispose();
    });
  }

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
    for (const { fixture, prior, address, notice, quoted } of [
      { fixture: "repeatedTableFooter", prior: "earlierTableReport", address: "123 Example Street", notice: "please unsubscribe", quoted: false },
      { fixture: "repeatedFlowFooter", prior: "earlierFlowReport", address: "456 Demonstration Avenue", notice: "unsubscribe from future updates", quoted: false },
      { fixture: "tableFooterWithEmptyQuote", prior: "earlierTableReport", address: "123 Example Street", notice: "please unsubscribe", quoted: false },
      { fixture: "flowFooterWithEmptyQuote", prior: "earlierFlowReport", address: "456 Demonstration Avenue", notice: "unsubscribe from future updates", quoted: false },
      { fixture: "tableFooterBeforeQuote", prior: "earlierTableReport", address: "123 Example Street", notice: "please unsubscribe", quoted: true },
      { fixture: "flowFooterBeforeQuote", prior: "earlierFlowReport", address: "456 Demonstration Avenue", notice: "unsubscribe from future updates", quoted: true },
    ]) {
      for (const width of [390, 1200]) {
        test(`${fixture} remains complete in a conversation in ${theme} mode at ${width}px`, async ({ page }) => {
          await page.setViewportSize({ width, height: 900 });
          await page.goto(`/tests/email-rendering.html?theme=${theme}&fixture=${fixture}&prior=${prior}`);
          const frame = page.locator("iframe.message-body");
          const body = frame.contentFrame().locator("body");
          const addressText = body.getByText(address, { exact: false });
          const noticeText = body.getByText(notice, { exact: false });
          await expect(addressText).toBeVisible();
          await expect(noticeText).toBeVisible();
          const toggle = page.getByRole("button", { name: "Show quoted content" });
          if (quoted) await expect(toggle).toBeVisible();
          else await expect(toggle).toHaveCount(0);
          const logo = body.locator("img").first();
          // The address stays beside the logo, and the entire notice fits
          // inside the frame rather than leaving a truncated footer.
          const logoBox = (await logo.boundingBox())!;
          const addressBox = (await addressText.boundingBox())!;
          expect(addressBox.x).toBeGreaterThanOrEqual(logoBox.x + logoBox.width);
          const frameBox = (await frame.boundingBox())!;
          const noticeBox = (await noticeText.boundingBox())!;
          expect(noticeBox.y + noticeBox.height).toBeLessThanOrEqual(frameBox.y + frameBox.height);
          if (quoted) {
            await expect(body).not.toContainText("Earlier quoted message.");
            await toggle.click();
            await expect(body).toContainText("Earlier quoted message.");
            await expect(addressText).toBeVisible();
            await page.getByRole("button", { name: "Hide quoted content" }).click();
            await expect(body).not.toContainText("Earlier quoted message.");
            await expect(addressText).toBeVisible();
            await expect(noticeText).toBeVisible();
          }
        });
      }
    }

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

  for (const { fixture, alt, imageWidth, imageHeight } of [
    { fixture: "iconCalloutFlexTable", alt: "info", imageWidth: 18, imageHeight: 18 },
    { fixture: "iconCalloutTable", alt: "note", imageWidth: 24, imageHeight: 24 },
    { fixture: "heightOnlyIconTable", alt: "mail", imageWidth: 20, imageHeight: 20 },
    { fixture: "heightOnlyIconFlex", alt: "notice", imageWidth: 40, imageHeight: 20 },
  ]) {
    for (const theme of ["light", "dark"]) {
      for (const width of [390, 1200]) {
        test(`${fixture} keeps its declared icon size beside wrapping text in ${theme} at ${width}px`, async ({ page }) => {
          await page.setViewportSize({ width, height: 900 });
          await page.goto(`/tests/email-rendering.html?theme=${theme}&images&fixture=${fixture}`);
          const content = page.locator("iframe.message-body").contentFrame();
          const icon = content.getByAltText(alt);
          await expect(icon).toHaveAttribute("src", /^data:image\//);
          await expect.poll(async () => (await icon.boundingBox())?.width).toBe(imageWidth);
          expect((await icon.boundingBox())!.height).toBe(imageHeight);
          if (fixture === "heightOnlyIconFlex") {
            for (const { name, cap } of [{ name: "inline cap", cap: 10 }, { name: "stylesheet cap", cap: 12 }]) {
              await expect.poll(async () => (await content.getByAltText(name).boundingBox())?.width).toBe(cap);
            }
          }
        });
      }
    }
  }

  test("legacy dimension attributes lay out exactly as the unsanitized HTML does", async ({ page, browser }) => {
    const measure = (root: Document) => Array.from(root.querySelectorAll("table, td"), (element) => {
      const style = root.defaultView!.getComputedStyle(element);
      return `${element.tagName} ${style.width}x${style.height} pad ${style.paddingLeft} spacing ${style.borderSpacing}`;
    });
    const reference = await browser.newPage();
    await reference.setContent(`<!doctype html><body style="margin:0">${emailRenderingFixtures.legacyDimensionAttributes}</body>`);
    const expected = await reference.evaluate(`(${measure.toString()})(document)`) as string[];
    await reference.close();

    await page.goto("/tests/email-rendering.html?theme=light&fixture=legacyDimensionAttributes");
    const frame = page.locator("iframe.message-body");
    await expect(frame.contentFrame().locator("td").first()).toBeAttached();
    const messageFrame = (await (await frame.elementHandle())!.contentFrame())!;
    const actual = await messageFrame.evaluate(`(${measure.toString()})(document)`) as string[];

    expect(actual).toHaveLength(expected.length);
    expect(actual).toEqual(expected);
  });

  for (const fixture of ["backgroundIconLinks", "backgroundPanelLonghands"] as const) {
    test(`${fixture} places and scales its background image as the unsanitized HTML does`, async ({ page, browser }) => {
      const measure = (root: Document) => Array.from(root.body.querySelectorAll("*"), (element) => {
        const style = root.defaultView!.getComputedStyle(element);
        if (style.backgroundImage === "none") return null;
        return `${element.tagName} ${style.width}x${style.height} size ${style.backgroundSize} position ${style.backgroundPosition} repeat ${style.backgroundRepeat} origin ${style.backgroundOrigin} clip ${style.backgroundClip}`;
      }).filter(Boolean);
      const reference = await browser.newPage();
      await reference.setContent(`<!doctype html><body style="margin:0">${emailRenderingFixtures[fixture]}</body>`);
      const expected = await reference.evaluate(`(${measure.toString()})(document)`) as string[];
      await reference.close();
      expect(expected.length).toBeGreaterThan(0);

      await page.goto(`/tests/email-rendering.html?theme=light&images&fixture=${fixture}`);
      const frame = page.locator("iframe.message-body");
      const messageFrame = (await (await frame.elementHandle())!.contentFrame())!;
      await expect.poll(() => messageFrame.evaluate(() => document.querySelectorAll("[data-blocked-src]").length)).toBe(0);
      await expect.poll(() => messageFrame.evaluate(`(${measure.toString()})(document)`)).toEqual(expected);
    });
  }

  for (const { fixture, alt, columnWidth, link } of [
    { fixture: "pxGutterFixedHeader", alt: "logo", columnWidth: 20, link: "Account" },
    { fixture: "pxIconColumnFixedTable", alt: "status", columnWidth: 56, link: "Open summary" },
  ]) {
    test(`${fixture} keeps px-suffixed column widths in a fixed-layout table`, async ({ page }) => {
      await page.setViewportSize({ width: 1200, height: 900 });
      await page.goto(`/tests/email-rendering.html?theme=light&images&fixture=${fixture}`);
      const frame = page.locator("iframe.message-body");
      const content = frame.contentFrame();
      await expect(content.getByAltText(alt)).toHaveAttribute("src", /^data:image\//);
      const pxColumn = content.locator("td[width]").first();
      await expect.poll(async () => (await pxColumn.boundingBox())?.width).toBe(columnWidth);
      const frameBox = (await frame.boundingBox())!;
      const linkBox = (await content.getByRole("link", { name: link }).boundingBox())!;
      expect(linkBox.x + linkBox.width).toBeLessThanOrEqual(frameBox.x + frameBox.width);
      const overflow = await frame.evaluate((element) => {
        const root = (element as HTMLIFrameElement).contentDocument!.documentElement;
        return root.scrollWidth - root.clientWidth;
      });
      expect(overflow).toBeLessThanOrEqual(0);
    });
  }

  for (const { fixture, width } of [
    { fixture: "centeredHybridColumn", width: 600 },
    { fixture: "centeredStyledTable", width: 480 },
  ] as const) {
    test(`${fixture} centers its column in a wide pane and fills a narrow one`, async ({ page }) => {
      const gaps = () => page.locator("iframe.message-body").contentFrame().locator(".centered").evaluate((element) => {
        const box = element.getBoundingClientRect();
        const parent = element.parentElement!.getBoundingClientRect();
        return { width: Math.round(box.width), left: Math.round(box.left - parent.left), right: Math.round(parent.right - box.right) };
      });
      await page.setViewportSize({ width: 1400, height: 900 });
      await page.goto(`/tests/email-rendering.html?theme=light&fixture=${fixture}`);
      await expect.poll(async () => (await gaps()).width).toBe(width);
      const wide = await gaps();
      expect(wide.left).toBeGreaterThan(0);
      expect(Math.abs(wide.left - wide.right)).toBeLessThanOrEqual(1);

      if (fixture === "centeredHybridColumn") {
        // max-width, not width: a pane narrower than the column fills it.
        await page.setViewportSize({ width: 390, height: 900 });
        await expect.poll(async () => (await gaps()).left).toBe(0);
        expect((await gaps()).right).toBe(0);
      }
    });
  }

  for (const { fixture, columnWidths, text } of [
    { fixture: "colWidthZeroWidthTable", columnWidths: [158, 601, 150], text: "300 USD" },
    { fixture: "colgroupWidthFixedTable", columnWidths: [160, 80, 240], text: "Ships next week" },
  ]) {
    test(`${fixture} sizes its columns from <col>/<colgroup> widths`, async ({ page }) => {
      await page.setViewportSize({ width: 1400, height: 900 });
      await page.goto(`/tests/email-rendering.html?theme=light&fixture=${fixture}`);
      const content = page.locator("iframe.message-body").contentFrame();
      const cells = content.locator("tr").first().locator("td");
      await expect(cells).toHaveCount(columnWidths.length);
      for (const [index, width] of columnWidths.entries()) {
        await expect.poll(async () => Math.round((await cells.nth(index).boundingBox())?.width ?? 0)).toBe(width);
      }
      // Collapsed columns wrap a word per line and clip it; sized columns keep
      // each row's text visible on a short row.
      const cell = content.getByText(text);
      await expect(cell).toBeVisible();
      const metrics = await cell.evaluate((element) => {
        const td = element.closest("td")!;
        return { clipped: td.scrollWidth - td.clientWidth, height: td.getBoundingClientRect().height };
      });
      expect(metrics.clipped).toBeLessThanOrEqual(0);
      expect(metrics.height).toBeLessThan(100);
    });
  }

  for (const { fixture, alt } of [
    { fixture: "heroInTable", alt: "hero" },
    { fixture: "heroInFlow", alt: "banner" },
  ]) {
    test(`${fixture} scales a wide image to fit a narrow pane`, async ({ page }) => {
      await page.setViewportSize({ width: 390, height: 900 });
      await page.goto(`/tests/email-rendering.html?theme=light&images&fixture=${fixture}`);
      const frame = page.locator("iframe.message-body");
      const image = frame.contentFrame().getByAltText(alt);
      await expect(image).toHaveAttribute("src", /^data:image\//);
      const frameBox = (await frame.boundingBox())!;
      await expect.poll(async () => (await image.boundingBox())?.width).toBeLessThan(600);
      const imageBox = (await image.boundingBox())!;
      expect(imageBox.x + imageBox.width).toBeLessThanOrEqual(frameBox.x + frameBox.width);
      const overflow = await frame.evaluate((element) => {
        const root = (element as HTMLIFrameElement).contentDocument!.documentElement;
        return root.scrollWidth - root.clientWidth;
      });
      expect(overflow).toBeLessThanOrEqual(0);
    });
  }

  for (const fixture of ["preformattedInFixedTable", "preformattedInFlow"]) {
    for (const width of [390, 1200]) {
      test(`${fixture} wraps long preformatted lines inside the pane at ${width}px`, async ({ page }) => {
        await page.setViewportSize({ width, height: 900 });
        await page.goto(`/tests/email-rendering.html?theme=light&fixture=${fixture}`);
        const frame = page.locator("iframe.message-body");
        const pre = frame.contentFrame().locator("pre");
        await expect(pre).toHaveCSS("white-space", "pre-wrap");
        const metrics = await pre.evaluate((element) => {
          const root = element.ownerDocument.documentElement;
          const lineHeight = parseFloat(getComputedStyle(element).lineHeight);
          return {
            preOverflow: element.scrollWidth - element.clientWidth,
            rootOverflow: root.scrollWidth - root.clientWidth,
            lines: Math.round(element.getBoundingClientRect().height / lineHeight),
            text: element.textContent ?? "",
          };
        });
        expect(metrics.preOverflow).toBeLessThanOrEqual(0);
        expect(metrics.rootOverflow).toBeLessThanOrEqual(0);
        // Every authored line break survives, and long lines add wrapped rows.
        const authoredLines = metrics.text.replace(/^\n/, "").replace(/\n\s*$/, "").split("\n").length;
        expect(metrics.lines).toBeGreaterThan(authoredLines);
      });
    }
  }

  test("author CSS keeps preformatted text unwrapped when it asks to", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 900 });
    await page.goto("/tests/email-rendering.html?theme=light&fixture=preformattedAuthorNoWrap");
    const pre = page.locator("iframe.message-body").contentFrame().locator("pre");
    await expect(pre).toHaveCSS("white-space", "pre");
    expect(await pre.evaluate((element) => element.scrollWidth > element.clientWidth)).toBe(true);
  });

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

  test("keeps the quoted-history toggle at the fold so it can refold", async ({ page }) => {
    await page.goto("/tests/email-rendering.html?theme=light&fixture=replyAttributionInsideCitation");
    const frame = page.locator("iframe.message-body");
    const body = frame.contentFrame().locator("body");
    const marker = frame.contentFrame().locator("span[data-quoted-history-fold]");
    const show = page.getByRole("button", { name: "Show quoted content" });
    await expect(body).toContainText("Sounds good.");

    const toggleTop = async (name: string) => (await page.getByRole("button", { name }).boundingBox())!.y;
    const markerTop = async () => (await marker.boundingBox())!.y;
    const collapsedTop = await toggleTop("Show quoted content");
    expect(Math.abs(collapsedTop - (await markerTop()) - 8)).toBeLessThanOrEqual(1);
    const collapsedFrameHeight = (await frame.boundingBox())!.height;

    await show.click();
    await expect(body).toContainText("Earlier message content.");
    await expect.poll(async () => (await frame.boundingBox())!.height).toBeGreaterThan(collapsedFrameHeight);
    expect(Math.abs((await toggleTop("Hide quoted content")) - collapsedTop)).toBeLessThanOrEqual(2);
    const quotedTop = await body.getByText("Earlier message content.").boundingBox();
    expect(quotedTop!.y).toBeGreaterThan(collapsedTop);

    await page.getByRole("button", { name: "Hide quoted content" }).click();
    await expect(body).not.toContainText("Earlier message content.");
    expect(Math.abs((await toggleTop("Show quoted content")) - collapsedTop)).toBeLessThanOrEqual(2);
  });

  test("folds a signature repeated from an earlier message along with the quoted history", async ({ page }) => {
    await page.goto("/tests/email-rendering.html?theme=light&fixture=replyRepeatingSignature&prior=earlierMessageWithSignature");
    const body = page.locator("iframe.message-body").contentFrame().locator("body");
    await expect(body).toContainText("Fixed now.");
    await expect(body).not.toContainText("Engineering Lead");
    await expect(body).not.toContainText("Earlier message content.");

    await page.getByRole("button", { name: "Show quoted content" }).click();
    await expect(body).toContainText("Engineering Lead");
    await expect(body).toContainText("Earlier message content.");
  });
});
