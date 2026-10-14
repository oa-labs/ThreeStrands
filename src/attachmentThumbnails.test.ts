import { describe, expect, it } from "vitest";
import type { MessageAttachment } from "./domain";
import { EMAIL_IMAGE_LIMITS } from "./emailRenderingPolicy";
import { thumbnailAttachmentIds } from "./attachmentThumbnails";

const image = (id: string, size: number, mimeType = "image/jpeg"): MessageAttachment => ({ id, filename: `${id}.jpg`, mimeType, size });

describe("thumbnailAttachmentIds", () => {
  it("previews supported rasters only, whatever the MIME parameters or case", () => {
    const ids = thumbnailAttachmentIds([
      image("png", 100, "image/png"),
      image("jpeg", 100, "IMAGE/JPEG; name=photo.jpg"),
      image("gif", 100, "image/gif"),
      image("webp", 100, "image/webp"),
      image("avif", 100, "image/avif"),
      image("svg", 100, "image/svg+xml"),
      image("heic", 100, "image/heic"),
      image("octet", 100, "application/octet-stream"),
      image("html", 100, "text/html"),
    ]);
    expect([...ids]).toEqual(["png", "jpeg", "gif", "webp", "avif"]);
  });

  it("previews a file below and at the per-file cap, never above it", () => {
    const max = EMAIL_IMAGE_LIMITS.maxAttachmentThumbnailBytes;
    expect([...thumbnailAttachmentIds([image("below", max - 1)])]).toEqual(["below"]);
    expect([...thumbnailAttachmentIds([image("exact", max)])]).toEqual(["exact"]);
    expect([...thumbnailAttachmentIds([image("above", max + 1)])]).toEqual([]);
  });

  it("skips empty and malformed sizes", () => {
    expect([...thumbnailAttachmentIds([image("zero", 0), image("negative", -1), image("nan", Number.NaN), image("infinite", Number.POSITIVE_INFINITY)])]).toEqual([]);
  });

  it("stops at the per-message count, previewing the first files in message order", () => {
    const count = EMAIL_IMAGE_LIMITS.maxAttachmentThumbnailsPerMessage;
    const below = Array.from({ length: count - 1 }, (_, index) => image(`p${index}`, 1_000));
    const exact = Array.from({ length: count }, (_, index) => image(`p${index}`, 1_000));
    const above = Array.from({ length: count + 1 }, (_, index) => image(`p${index}`, 1_000));
    expect(thumbnailAttachmentIds(below).size).toBe(count - 1);
    expect(thumbnailAttachmentIds(exact).size).toBe(count);
    expect([...thumbnailAttachmentIds(above)]).toEqual(exact.map((attachment) => attachment.id));
  });

  it("counts each preview at its encoded data-URI size against the message budget", () => {
    const budget = EMAIL_IMAGE_LIMITS.maxDataUriBytesPerMessage;
    // 3 raw bytes encode to 4, so a file of budget × 3/4 bytes fills the budget exactly.
    const fill = (budget / 4) * 3;
    const per = Math.floor(EMAIL_IMAGE_LIMITS.maxAttachmentThumbnailBytes / 3) * 3;
    const files: MessageAttachment[] = [];
    let remaining = fill;
    while (remaining > 0) {
      const size = Math.min(per, remaining);
      files.push(image(`f${files.length}`, size));
      remaining -= size;
    }
    expect(files.length).toBeLessThan(EMAIL_IMAGE_LIMITS.maxAttachmentThumbnailsPerMessage);
    expect(thumbnailAttachmentIds(files).size).toBe(files.length);
    expect(thumbnailAttachmentIds([...files, image("over", 3)]).has("over")).toBe(false);
    expect(thumbnailAttachmentIds([...files.slice(0, -1), image("under", files.at(-1)!.size - 3)]).has("under")).toBe(true);
  });

  it("keeps previewing smaller files after one would overrun the budget", () => {
    const max = EMAIL_IMAGE_LIMITS.maxAttachmentThumbnailBytes;
    const files = [image("a", max), image("b", max), image("c", max), image("big", max), image("small", 3)];
    const ids = thumbnailAttachmentIds(files);
    expect(ids.has("big")).toBe(false);
    expect(ids.has("small")).toBe(true);
  });
});
