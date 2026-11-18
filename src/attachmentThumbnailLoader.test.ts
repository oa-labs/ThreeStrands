import { afterEach, describe, expect, it, vi } from "vitest";
import { loadAttachmentThumbnail, resizeAttachmentThumbnail } from "./attachmentThumbnailLoader";
import { mailClient } from "./data/client";
import { EMAIL_IMAGE_LIMITS } from "./emailRenderingPolicy";

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });
describe("attachment thumbnail loading", () => {
  it("shares only in-flight thumbnail requests, and never requests an inline original", async () => {
    let finish!: (value: { dataUri: string; needsResize: boolean }) => void;
    const get = vi.spyOn(mailClient, "fetchAttachmentThumbnail").mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    const original = vi.spyOn(mailClient, "fetchAttachmentImage");
    const first = loadAttachmentThumbnail("message", "photo");
    expect(loadAttachmentThumbnail("message", "photo")).toBe(first);
    finish({ dataUri: "data:image/png;base64,AAAA", needsResize: false });
    await expect(first).resolves.toBe("data:image/png;base64,AAAA");
    get.mockResolvedValue({ dataUri: null, needsResize: false });
    await loadAttachmentThumbnail("message", "photo");
    expect(get).toHaveBeenCalledTimes(2);
    expect(original).not.toHaveBeenCalled();
  });

  it("bounds work across messages and rejects excess queued work", async () => {
    const releases: (() => void)[] = [];
    const get = vi.spyOn(mailClient, "fetchAttachmentThumbnail").mockImplementation(() => new Promise((resolve) => releases.push(() => resolve({ dataUri: null, needsResize: false }))));
    const count = EMAIL_IMAGE_LIMITS.maxConcurrentAttachmentThumbnails + EMAIL_IMAGE_LIMITS.maxPendingGlobally;
    const pending = Array.from({ length: count }, (_, i) => loadAttachmentThumbnail("many", String(i)));
    expect(get).toHaveBeenCalledTimes(EMAIL_IMAGE_LIMITS.maxConcurrentAttachmentThumbnails);
    await expect(loadAttachmentThumbnail("many", "overflow")).rejects.toThrow("queue is full");
    for (let i = 0; i < count; i++) {
      releases[i](); await pending[i]; await Promise.resolve();
    }
    await Promise.all(pending);
  });

  it.each(["https://tracker.invalid/pixel.png", "data:image/svg+xml;base64,AAAA", "data:text/html;base64,AAAA"])("refuses an unsafe thumbnail response: %s", async (dataUri) => {
    vi.spyOn(mailClient, "fetchAttachmentThumbnail").mockResolvedValue({ dataUri, needsResize: false });
    await expect(loadAttachmentThumbnail("unsafe", dataUri)).rejects.toThrow("Invalid attachment thumbnail");
    await expect(resizeAttachmentThumbnail(dataUri)).rejects.toThrow("Unsupported attachment image");
  });

  it("checks the encoded budget below, at, and above the limit", async () => {
    const prefix = "data:image/png;base64,";
    const max = EMAIL_IMAGE_LIMITS.maxAttachmentThumbnailDataUriBytes;
    const get = vi.spyOn(mailClient, "fetchAttachmentThumbnail");
    for (const length of [max - 1, max, max + 1]) {
      get.mockResolvedValue({ dataUri: prefix + "A".repeat(length - prefix.length), needsResize: false });
      const result = loadAttachmentThumbnail("budget", String(length));
      if (length <= max) await expect(result).resolves.toHaveLength(length);
      else await expect(result).rejects.toThrow("Invalid attachment thumbnail");
    }
  });

  it.each([[4000, 3000, 40, 30], [200, 6000, 1, 40], [12, 8, 12, 8]])("downsamples %sx%s to %sx%s and releases the source", async (width, height, expectedWidth, expectedHeight) => {
    const instances: { src: string; onload: (() => void) | null; onerror: (() => void) | null }[] = [];
    vi.stubGlobal("Image", class {
      naturalWidth = width; naturalHeight = height;
      onload: (() => void) | null = null; onerror: (() => void) | null = null;
      value = "";
      constructor() { instances.push(this); }
      set src(value: string) { this.value = value; if (value) queueMicrotask(() => this.onload?.()); }
      get src() { return this.value; }
    });
    const drawImage = vi.fn();
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ drawImage } as unknown as CanvasRenderingContext2D);
    const toData = vi.spyOn(HTMLCanvasElement.prototype, "toDataURL").mockImplementation(function (this: HTMLCanvasElement) {
      expect([this.width, this.height]).toEqual([expectedWidth, expectedHeight]);
      return "data:image/png;base64,AAAA";
    });
    expect(await resizeAttachmentThumbnail("data:image/jpeg;base64,AAAA")).toBe("data:image/png;base64,AAAA");
    expect(drawImage).toHaveBeenCalledWith(instances[0], 0, 0, expectedWidth, expectedHeight);
    expect((toData.mock.instances[0] as HTMLCanvasElement).width).toBe(0);
    expect(instances[0].src).toBe("");
  });
});
