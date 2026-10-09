import { afterEach, describe, expect, it, vi } from "vitest";
import { openUrl } from "@tauri-apps/plugin-opener";
import { isMailtoLink, openMessageLink, parseMailto, setMailtoHandler } from "./mailtoLink";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const empty = { to: "", cc: "", bcc: "", subject: "", body: "" };

describe("parseMailto", () => {
  it("reads every supported field", () => {
    expect(parseMailto(
      "mailto:jane@example.com?cc=alex@example.com&bcc=audit@example.com&subject=Quarterly%20plan&body=Hi%20Jane%2C%0D%0A%0D%0ASee%20below.",
    )).toEqual({
      to: "jane@example.com",
      cc: "alex@example.com",
      bcc: "audit@example.com",
      subject: "Quarterly plan",
      body: "Hi Jane,\n\nSee below.",
    });
  });

  it("opens an empty draft for a bare scheme", () => {
    expect(parseMailto("mailto:")).toEqual(empty);
  });

  it("matches the scheme and field names without regard to case", () => {
    expect(parseMailto("MAILTO:jane@example.com?SUBJECT=Hi&Cc=alex@example.com")).toEqual({
      ...empty, to: "jane@example.com", cc: "alex@example.com", subject: "Hi",
    });
  });

  it("combines addresses from the path, to= fields, and encoded commas without repeats", () => {
    expect(parseMailto("mailto:a@example.com,b@example.com?to=c@example.com%2CA@example.com&to=d@example.com&cc=e@example.com&cc=f@example.com")).toEqual({
      ...empty,
      to: "a@example.com, b@example.com, c@example.com, d@example.com",
      cc: "e@example.com, f@example.com",
    });
  });

  it("keeps a quoted display name with a comma as one recipient", () => {
    expect(parseMailto("mailto:%22Doe%2C%20Jane%22%20%3Cjane@example.com%3E")?.to).toBe('"Doe, Jane" <jane@example.com>');
  });

  it("treats + as a literal plus, not a space", () => {
    expect(parseMailto("mailto:jane+news@example.com?subject=1+1")).toEqual({
      ...empty, to: "jane+news@example.com", subject: "1+1",
    });
  });

  it("uses the first subject and body when a link repeats them", () => {
    expect(parseMailto("mailto:a@example.com?subject=First&subject=Second&body=One&body=Two")).toEqual({
      ...empty, to: "a@example.com", subject: "First", body: "One",
    });
  });

  it("keeps a malformed escape as literal text instead of dropping the field", () => {
    expect(parseMailto("mailto:a@example.com?subject=100%25%20done%zz")?.subject).toBe("100% done%zz");
  });

  it("keeps an equals sign inside a field value", () => {
    expect(parseMailto("mailto:a@example.com?body=x=1")?.body).toBe("x=1");
  });

  it.each([
    ["attach", "mailto:a@example.com?attach=file:///Users/me/.ssh/id_ed25519"],
    ["attachment", "mailto:a@example.com?attachment=%2Fetc%2Fpasswd"],
    ["from", "mailto:a@example.com?from=ceo@example.com"],
    ["in-reply-to", "mailto:a@example.com?in-reply-to=%3Cmessage@example.com%3E"],
    ["unknown headers", "mailto:a@example.com?x-mailer=Evil&keywords=urgent"],
  ])("ignores %s", (_name, href) => {
    expect(parseMailto(href)).toEqual({ ...empty, to: "a@example.com" });
  });

  it("cannot inject headers through line breaks in addresses or the subject", () => {
    expect(parseMailto("mailto:a@example.com%0D%0ABcc:%20spy@example.com?cc=b@example.com%0Aspy@example.com&subject=Hi%0D%0ABcc:%20spy@example.com")).toEqual({
      ...empty,
      to: "a@example.com Bcc: spy@example.com",
      cc: "b@example.com spy@example.com",
      subject: "Hi Bcc: spy@example.com",
    });
  });

  it.each(["https://example.com/", "tel:+15551234567", "javascript:alert(1)", "file:///etc/passwd", "mailto", " xmailto:a@example.com"])(
    "returns null for %s",
    (href) => {
      expect(parseMailto(href)).toBeNull();
      expect(isMailtoLink(href)).toBe(false);
    },
  );
});

describe("openMessageLink", () => {
  afterEach(() => {
    setMailtoHandler(null);
    vi.mocked(openUrl).mockClear();
  });

  it("sends mailto links to the composer and everything else to the OS browser", () => {
    const handler = vi.fn();
    setMailtoHandler(handler);

    openMessageLink("mailto:jane@example.com?subject=Hello");
    openMessageLink("https://example.com/story");

    expect(handler).toHaveBeenCalledExactlyOnceWith({ ...empty, to: "jane@example.com", subject: "Hello" });
    expect(openUrl).toHaveBeenCalledExactlyOnceWith("https://example.com/story");
  });

  it("holds links that arrive before the composer is ready and delivers them in order", () => {
    openMessageLink("mailto:first@example.com");
    openMessageLink("mailto:second@example.com");
    const handler = vi.fn();

    setMailtoHandler(handler);

    expect(handler.mock.calls.map(([request]) => request.to)).toEqual(["first@example.com", "second@example.com"]);
    expect(openUrl).not.toHaveBeenCalled();
  });
});
