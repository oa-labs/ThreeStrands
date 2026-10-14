import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clearFilePreviewCache, FilePreview, wantsFilePreview } from "./FilePreview";
import { mailClient } from "./data/client";
import type { WorkspaceFile } from "./domain";
import { FILE_PREVIEW_LIMITS } from "./emailRenderingPolicy";

const slides: WorkspaceFile = { accountId: "you@example.com", messageId: "m-slides", threadId: "t-slides", subject: "Intro", sentAt: "2026-10-14T18:34:00Z", sender: "Ronnie <ronnie@example.com>", direction: "received", attachment: { id: "slides", filename: "Slides.pdf", mimeType: "application/pdf", size: 1_700_000 }, starred: false, availability: "download" };
const sheet: WorkspaceFile = { ...slides, messageId: "m-sheet", attachment: { id: "sheet", filename: "forecast.xlsx", mimeType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", size: 40_000 } };
const PNG = "data:image/png;base64,iVBORw0KGgo=";
const sized = (size: number): WorkspaceFile => ({ ...slides, attachment: { ...slides.attachment, size } });

describe("FilePreview", () => {
  afterEach(() => { cleanup(); vi.restoreAllMocks(); clearFilePreviewCache(); });

  it("shows the native preview and opens the file when clicked", async () => {
    const preview = vi.spyOn(mailClient, "filePreview").mockResolvedValue(PNG);
    const onOpen = vi.fn();
    const { container } = render(<FilePreview file={slides} online onOpen={onOpen} />);
    await waitFor(() => expect(container.querySelector("img")).toHaveAttribute("src", PNG));
    expect(preview).toHaveBeenCalledWith("m-slides", "slides");
    // The Open button is the accessible route; the picture is decorative.
    expect(container.querySelector(".file-detail-preview")).toHaveAttribute("aria-hidden", "true");
    fireEvent.click(container.querySelector(".file-detail-preview")!);
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("renders nothing for a file without a preview, a failed request, or anything but a PNG data URI", async () => {
    for (const result of [null, "data:image/svg+xml;base64,PHN2Zz4=", "https://example.com/x.png"]) {
      vi.spyOn(mailClient, "filePreview").mockResolvedValue(result);
      const { container, unmount } = render(<FilePreview file={slides} online onOpen={vi.fn()} />);
      await waitFor(() => expect(mailClient.filePreview).toHaveBeenCalled());
      expect(container).toBeEmptyDOMElement();
      unmount(); clearFilePreviewCache(); vi.restoreAllMocks();
    }
    vi.spyOn(mailClient, "filePreview").mockRejectedValue(new Error("Attachment data is unavailable"));
    const { container } = render(<FilePreview file={slides} online onOpen={vi.fn()} />);
    await waitFor(() => expect(mailClient.filePreview).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it("never asks for a preview of a risky file", () => {
    const preview = vi.spyOn(mailClient, "filePreview").mockResolvedValue(PNG);
    for (const filename of ["invoice.pdf.exe", "Secure Message.html", "Budget.xlsm"]) {
      render(<FilePreview file={{ ...slides, attachment: { ...slides.attachment, filename } }} online onOpen={vi.fn()} />);
    }
    expect(preview).not.toHaveBeenCalled();
  });

  it("asks for files up to the native size cap and not beyond it", () => {
    const cap = FILE_PREVIEW_LIMITS.maxSourceBytes;
    expect(wantsFilePreview(sized(cap - 1), true)).toBe(true);
    expect(wantsFilePreview(sized(cap), true)).toBe(true);
    expect(wantsFilePreview(sized(cap + 1), true)).toBe(false);
    expect(wantsFilePreview(sized(0), true)).toBe(false);
  });

  it("matches the native preview size cap", () => {
    const source = readFileSync(resolve(process.cwd(), "src-tauri/src/file_preview.rs"), "utf8");
    const native = /PREVIEW_MAX_SOURCE_BYTES: usize = (\d+) \* 1024 \* 1024;/.exec(source)?.[1];
    expect(Number(native) * 1024 * 1024).toBe(FILE_PREVIEW_LIMITS.maxSourceBytes);
  });

  it("skips files that must be downloaded while offline, but previews ones kept on this device", () => {
    expect(wantsFilePreview(slides, false)).toBe(false);
    expect(wantsFilePreview({ ...slides, availability: "offline" }, false)).toBe(true);
  });

  it("reuses a recent preview when returning to a file, and keeps at most the cache limit", async () => {
    const preview = vi.spyOn(mailClient, "filePreview").mockResolvedValue(PNG);
    const view = render(<FilePreview file={slides} online onOpen={vi.fn()} />);
    await waitFor(() => expect(view.container.querySelector("img")).not.toBeNull());
    view.rerender(<FilePreview file={sheet} online onOpen={vi.fn()} />);
    await waitFor(() => expect(preview).toHaveBeenCalledTimes(2));
    view.rerender(<FilePreview file={slides} online onOpen={vi.fn()} />);
    expect(view.container.querySelector("img")).toHaveAttribute("src", PNG);
    expect(preview).toHaveBeenCalledTimes(2);

    for (let index = 0; index < FILE_PREVIEW_LIMITS.maxCachedPreviews; index += 1) {
      view.rerender(<FilePreview file={{ ...slides, messageId: `m-${index}` }} online onOpen={vi.fn()} />);
      await waitFor(() => expect(preview).toHaveBeenCalledTimes(3 + index));
    }
    view.rerender(<FilePreview file={slides} online onOpen={vi.fn()} />);
    await waitFor(() => expect(preview).toHaveBeenCalledTimes(3 + FILE_PREVIEW_LIMITS.maxCachedPreviews));
  });

  it("drops a stale preview at once when another file is selected", async () => {
    let resolveSheet: (value: string | null) => void = () => {};
    vi.spyOn(mailClient, "filePreview").mockImplementation((messageId) =>
      messageId === "m-slides" ? Promise.resolve(PNG) : new Promise((done) => { resolveSheet = done; }));
    const view = render(<FilePreview file={slides} online onOpen={vi.fn()} />);
    await waitFor(() => expect(view.container.querySelector("img")).not.toBeNull());
    view.rerender(<FilePreview file={sheet} online onOpen={vi.fn()} />);
    expect(view.container).toBeEmptyDOMElement();
    resolveSheet(null);
  });
});
