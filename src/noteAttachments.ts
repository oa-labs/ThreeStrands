import { mailClient } from "./data/client";
import { formatAttachmentSize } from "./threadPresentation";

/** How a note's Markdown names an attached file: `attachment:<id>`. */
export const ATTACHMENT_SCHEME = "attachment:";

/** The largest file a note can hold, as natively (`MAX_NOTE_ATTACHMENT_BYTES`). */
export const MAX_NOTE_ATTACHMENT_BYTES = 20 * 1024 * 1024;

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
 * The files of the note `noteId`. A failure is reported through `onError`.
 * An image another device added that hasn't downloaded yet waits, and
 * appears once `retry` finds it here (call it after each sync).
 */
export function noteFiles(noteId: () => string | null, onError: (message: string) => void): NoteFiles & { retry(): void } {
  const images = new Map<string, Promise<string | undefined>>();
  const waiting = new Map<string, Array<(url: string | undefined) => void>>();
  const message = (reason: unknown) => reason instanceof Error ? reason.message : String(reason);

  const load = (id: string): Promise<string | null> => mailClient.noteAttachmentImage(id);
  const waitFor = (id: string) => new Promise<string | undefined>((resolve) => {
    waiting.set(id, [...waiting.get(id) ?? [], resolve]);
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
        onError(message(reason));
        return undefined;
      }
    },
    image(src) {
      const id = attachmentIdOf(src);
      if (!id) return Promise.resolve(undefined);
      let found = images.get(id);
      if (!found) {
        found = load(id).then(async (url) => {
          if (url) return url;
          const attachment = await mailClient.noteAttachment(id);
          // Not here yet: show it once sync brings it. Anything else isn't a picture to show.
          if (attachment && attachment.image && !attachment.available) return waitFor(id);
          return undefined;
        }).catch(() => undefined);
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
      if (id) void mailClient.openNoteAttachment(id).catch((reason) => onError(message(reason)));
    },
    retry() {
      for (const [id, resolvers] of waiting) {
        void load(id).then((url) => {
          if (!url) return;
          waiting.delete(id);
          for (const resolve of resolvers) resolve(url);
        }, () => undefined);
      }
    },
  };
}
