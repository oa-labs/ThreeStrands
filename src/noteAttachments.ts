import { mailClient } from "./data/client";
import { errorMessage, logBackgroundFailure } from "./errors";
import { formatAttachmentSize } from "./threadPresentation";
import { MAX_NOTE_ATTACHMENT_BYTES } from "./addToNotePolicy";

export { MAX_NOTE_ATTACHMENT_BYTES } from "./addToNotePolicy";

/** How a note's Markdown names an attached file: `attachment:<id>`. */
export const ATTACHMENT_SCHEME = "attachment:";

/** The attachment id a Markdown destination names, or null. */
export function attachmentIdOf(destination: string): string | null {
  return /^attachment:([0-9a-f]{32})$/.exec(destination)?.[1] ?? null;
}

/** What the editor needs to store, show, and open a note's files. */
export type NoteFiles = {
  /** Stores a pasted or dropped file; resolves to its Markdown destination. */
  save(file: File): Promise<string | undefined>;
  /** A URL showing the image a destination names, once this device has it. */
  image(src: string): Promise<string | undefined>;
  /** The size shown on a file's pill. */
  info(href: string): Promise<{ size?: number } | undefined>;
  /** Opens the file a destination names. */
  open(href: string): void;
};

/**
 * Reports failures to save or open files through `onError`.
 * Images that are still downloading or could not be read wait for `retry`
 * after sync, so the existing editor image can appear as soon as it's ready.
 */
export function noteFiles(noteId: () => string | null, onError: (message: string) => void): NoteFiles & { retry(): void } {
  const images = new Map<string, Promise<string | undefined>>();
  const waiting = new Set<() => Promise<void>>();
  const reportImageFailure = logBackgroundFailure("Note attachment image");

  const readImage = (id: string) => new Promise<string | undefined>((resolve) => {
    const read = async () => {
      waiting.delete(read);
      try {
        const url = await mailClient.noteAttachmentImage(id);
        if (url) { resolve(url); return; }
        const attachment = await mailClient.noteAttachment(id);
        if (!(attachment?.image && !attachment.available)) { resolve(undefined); return; }
      } catch (reason) {
        reportImageFailure(reason);
      }
      waiting.add(read);
    };
    void read();
  });

  return {
    async save(file) {
      const id = noteId();
      if (!id) return undefined;
      if (file.size > MAX_NOTE_ATTACHMENT_BYTES) {
        onError(`“${file.name}” is ${formatAttachmentSize(file.size)}. A note can hold files up to 20 MB.`);
        return undefined;
      }
      try {
        const attachment = await mailClient.addNoteAttachment(id, file);
        return `${ATTACHMENT_SCHEME}${attachment.id}`;
      } catch (reason) {
        onError(errorMessage(reason));
        return undefined;
      }
    },
    image(src) {
      const id = attachmentIdOf(src);
      if (!id) return Promise.resolve(undefined);
      let found = images.get(id);
      if (!found) {
        found = readImage(id);
        images.set(id, found);
      }
      return found;
    },
    async info(href) {
      const id = attachmentIdOf(href);
      if (!id) return undefined;
      const attachment = await mailClient.noteAttachment(id).catch(() => null);
      return attachment ? { size: attachment.size } : undefined;
    },
    open(href) {
      const id = attachmentIdOf(href);
      if (id) void mailClient.openNoteAttachment(id).catch((reason) => onError(errorMessage(reason)));
    },
    retry() {
      for (const read of waiting) void read();
    },
  };
}
