import { afterEach, describe, expect, it, vi } from "vitest";
import { mailClient } from "./data/client";
import type { NoteAttachment } from "./domain";
import { MAX_NOTE_ATTACHMENT_BYTES, attachmentIdOf, noteFiles } from "./noteAttachments";

const ID = "0123456789abcdef0123456789abcdef";
const PIXEL = "data:image/png;base64,iVBORw0KGgo=";
const attachment = (changes: Partial<NoteAttachment> = {}): NoteAttachment => ({ id: ID, noteId: "n-1", name: "a.png", mimeType: "image/png", size: 3, image: true, available: true, ...changes });

/** A file whose size is reported without holding that many bytes. */
function fileOfSize(size: number, name = "big.bin") {
  const file = new File(["x"], name);
  Object.defineProperty(file, "size", { value: size });
  return file;
}

describe("note files", () => {
  afterEach(() => vi.restoreAllMocks());

  it("name attachments only by a whole attachment id", () => {
    expect(attachmentIdOf(`attachment:${ID}`)).toBe(ID);
    for (const other of [`attachment:${ID.toUpperCase()}`, `attachment:${ID}0`, "attachment:", `https://x/${ID}`, ID]) expect(attachmentIdOf(other)).toBeNull();
  });

  it("store a file in the open note and refuse one over the size limit without reading it", async () => {
    const add = vi.spyOn(mailClient, "addNoteAttachment").mockResolvedValue(attachment());
    const onError = vi.fn();
    const files = noteFiles(() => "n-1", onError);
    const atLimit = fileOfSize(MAX_NOTE_ATTACHMENT_BYTES);
    expect(await files.save(atLimit)).toBe(`attachment:${ID}`);
    expect(add).toHaveBeenCalledWith("n-1", atLimit);
    expect(await files.save(fileOfSize(MAX_NOTE_ATTACHMENT_BYTES + 1))).toBeUndefined();
    expect(add).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith(expect.stringContaining("up to 20 MB"));
  });

  it("report a file that couldn't be stored or opened", async () => {
    vi.spyOn(mailClient, "addNoteAttachment").mockRejectedValue(new Error("Notes already hold 2,000 files"));
    vi.spyOn(mailClient, "openNoteAttachment").mockRejectedValue(new Error("This file hasn't reached this device yet."));
    const onError = vi.fn();
    const files = noteFiles(() => "n-1", onError);
    expect(await files.save(new File(["x"], "a.txt"))).toBeUndefined();
    expect(onError).toHaveBeenLastCalledWith("Notes already hold 2,000 files");
    files.open(`attachment:${ID}`);
    await vi.waitFor(() => expect(onError).toHaveBeenLastCalledWith("This file hasn't reached this device yet."));
    await expect(noteFiles(() => null, onError).save(new File(["x"], "a.txt"))).resolves.toBeUndefined();
  });

  it("show an image once sync brings it, and nothing for other sources", async () => {
    const image = vi.spyOn(mailClient, "noteAttachmentImage").mockResolvedValue(null);
    const lookup = vi.spyOn(mailClient, "noteAttachment").mockResolvedValue(attachment({ available: false }));
    const files = noteFiles(() => "n-1", vi.fn());
    expect(await files.image("https://example.com/a.png")).toBeUndefined();
    let shown: string | undefined;
    void files.image(`attachment:${ID}`).then((url) => { shown = url; });
    await vi.waitFor(() => expect(lookup).toHaveBeenCalledTimes(1));
    await new Promise((resolve) => setTimeout(resolve, 0));
    files.retry();
    await vi.waitFor(() => expect(image).toHaveBeenCalledTimes(2));
    expect(shown).toBeUndefined();
    image.mockResolvedValue(PIXEL);
    files.retry();
    await vi.waitFor(() => expect(shown).toBe(PIXEL));
  });

  it("don't wait for a file that isn't a picture", async () => {
    vi.spyOn(mailClient, "noteAttachmentImage").mockResolvedValue(null);
    vi.spyOn(mailClient, "noteAttachment").mockResolvedValue(attachment({ image: false, mimeType: "application/pdf", available: false }));
    expect(await noteFiles(() => "n-1", vi.fn()).image(`attachment:${ID}`)).toBeUndefined();
  });
});
