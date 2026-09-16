import { describe, expect, it } from "vitest";
import { isInlineImageAttachment, normalizeContentId, referencedImageContentIds } from "./inlineAttachments";

const image = {
  id: "image-1",
  filename: "image.png",
  mimeType: "image/png",
  size: 42,
  contentId: "chart.one",
  inline: false,
};

describe("inline image attachments", () => {
  it("recognizes encoded and bracketed cid image references from cached messages", () => {
    const referenced = referencedImageContentIds('<p>Chart</p><img src="CID:chart%2Eone">');

    expect(referenced).toEqual(new Set(["chart.one"]));
    expect(normalizeContentId(" <Chart.One> ")).toBe("chart.one");
    expect(isInlineImageAttachment(image, referenced)).toBe(true);
  });

  it("does not hide files based on text, remote images, or non-image cid parts", () => {
    (globalThis as typeof globalThis & { inlineAttachmentScriptRan?: boolean }).inlineAttachmentScriptRan = false;
    const referenced = referencedImageContentIds(
      '<script>globalThis.inlineAttachmentScriptRan = true</script>'
      + '<p>Diagnostic cid:chart.one</p><img src="https://example.com/chart.png" onerror="globalThis.inlineAttachmentScriptRan = true">',
    );

    expect(referenced.size).toBe(0);
    expect((globalThis as typeof globalThis & { inlineAttachmentScriptRan?: boolean }).inlineAttachmentScriptRan).toBe(false);
    expect(isInlineImageAttachment(image, referenced)).toBe(false);
    expect(isInlineImageAttachment(
      { ...image, mimeType: "application/pdf" },
      new Set(["chart.one"]),
    )).toBe(false);
  });
});
