import { describe, expect, it } from "vitest";
import { DISGUISED_WORD_MIN_LETTERS, disguisedWords, knownSites, LOOKALIKE_MIN_TYPO_LENGTH, lookalikeOf, mixesScripts, skeleton, spelledOutSite, unicodeHost } from "./lookalike";

/** The ASCII form the platform's own URL parser gives a host. */
const ascii = (host: string) => new URL(`http://${host}`).hostname;

describe("international hosts", () => {
  it("decodes every Punycode label the URL parser writes, and leaves others alone", () => {
    for (const host of ["münchen.de", "pаrtner.example", "магазин.example", "аррӏе.com", "例え.テスト"]) {
      expect(unicodeHost(ascii(host)), host).toBe(host.toLocaleLowerCase());
    }
    expect(unicodeHost("Mail.Example.COM")).toBe("mail.example.com");
    expect(unicodeHost("xn--!!.example")).toBe("xn--!!.example");
  });

  it("finds labels that mix alphabets, but not a name written in one", () => {
    expect(mixesScripts("pаrtner")).toBe(true);
    expect(mixesScripts("ѕhop")).toBe(true);
    expect(mixesScripts("магазин")).toBe(false);
    expect(mixesScripts("münchen")).toBe(false);
    expect(mixesScripts("例え")).toBe(false);
  });
});

describe("look-alike sites", () => {
  const known = knownSites(["partner.example", "example.co.uk", "modern.example", "ibm.example"]);

  it("folds letters that read alike to one shape", () => {
    for (const value of ["examp1e", "exаmple", "ＥＸＡＭＰＬＥ", "éxample", "examplе"]) expect(skeleton(value), value).toBe(skeleton("example"));
    expect(skeleton("modern")).toBe(skeleton("modem"));
    expect(skeleton("vvave")).toBe(skeleton("wave"));
    expect(skeleton("partner")).not.toBe(skeleton("painter"));
  });

  it("matches a site that reads like a known one", () => {
    expect(lookalikeOf("partn3r.example", known)).toBe("partner.example");
    expect(lookalikeOf(ascii("pаrtner.example"), known)).toBe("partner.example");
    expect(lookalikeOf("modem.example", known)).toBe("modern.example");
    expect(lookalikeOf("examp1e.co.uk", known)).toBe("example.co.uk");
  });

  it("matches one added, dropped, or swapped letter in a long enough name under the same suffix", () => {
    expect(lookalikeOf("partnerr.example", known)).toBe("partner.example");
    expect(lookalikeOf("parner.example", known)).toBe("partner.example");
    expect(lookalikeOf("parnter.example", known)).toBe("partner.example");
    expect(lookalikeOf("partner.test", known)).toBeNull();
    // A different letter in place is a different word, not a typo: painter, partier.
    expect(lookalikeOf("partier.example", known)).toBeNull();
    expect("ibm".length).toBeLessThan(LOOKALIKE_MIN_TYPO_LENGTH);
    expect(lookalikeOf("ibmm.example", known)).toBeNull();
  });

  it("never calls a known site a look-alike of another", () => {
    const both = knownSites(["partner.example", "partnerr.example"]);
    expect(lookalikeOf("partnerr.example", both)).toBeNull();
    expect(lookalikeOf("PARTNER.example", known)).toBeNull();
  });

  it("finds a known site spelled out in front of the real one", () => {
    expect(spelledOutSite("partner.example.login.other.test", "other.test", known)).toBe("partner.example");
    expect(spelledOutSite("www.partner.example", "partner.example", known)).toBeNull();
    expect(spelledOutSite("partner.other.test", "other.test", known)).toBeNull();
  });
});

describe("look-alike words in text", () => {
  const tricks = (text: string) => disguisedWords(text).map(({ word, trick }) => `${word}: ${trick}`);

  it("finds a lowercase l hidden among capitals, between them or at the end", () => {
    expect(tricks("COMPLETE REVlEW TO REClEVE PAYOUTS")).toEqual(["REVlEW: a lowercase l in place of I", "REClEVE: a lowercase l in place of I"]);
    expect(tricks("Your PAYPAl account")).toEqual(["PAYPAl: a lowercase l in place of I"]);
  });

  it("finds a capital I closing a small-letter word", () => {
    expect(tricks("Update your paypaI details")).toEqual(["paypaI: a capital I in place of l"]);
  });

  it("finds digits drawn like letters between letters, in the word's own case", () => {
    expect(tricks("Verify your ACC0UNT")).toEqual(["ACC0UNT: the digit 0 in place of O"]);
    expect(tricks("Micr0soft notice")).toEqual(["Micr0soft: the digit 0 in place of o"]);
    expect(tricks("B1LLING problem")).toEqual([]);
    expect(tricks("BlLLING problem")).toEqual(["BlLLING: a lowercase l in place of I"]);
    expect(tricks("Your 5ecurity c0de")).toEqual([]);
  });

  it("finds letters from another alphabet mixed into a word", () => {
    expect(tricks("Your pаypal invoice")).toEqual(["pаypal: a Cyrillic letter in place of a Latin one"]);
    expect(tricks("Ѕесurity alert")).toEqual(["Ѕесurity: Cyrillic letters in place of Latin ones"]);
    expect(tricks("Rеvіеw")).toEqual(["Rеvіеw: Cyrillic letters in place of Latin ones"]);
    expect(tricks("Αccount locked")).toEqual(["Αccount: a Greek letter in place of a Latin one"]);
  });

  it("finds invisible characters that split a word, and shows the word without them", () => {
    expect(tricks("Pay\u200Bpal refund")).toEqual(["Paypal: an invisible character between letters"]);
    expect(tricks("Re\u00ADfund ready")).toEqual(["Refund: an invisible character between letters"]);
    expect(tricks("We\u2060lcome")).toEqual(["Welcome: an invisible character between letters"]);
  });

  it("leaves ordinary subjects alone across languages, brands, codes, and versions", () => {
    const ordinary = [
      "Your iPhone order has shipped", "McDonald's receipt", "Mr. McIntosh's visit", "Q3 OKRs", "COVID-19 update",
      "Windows 10 and Office365", "H1N1 and B2B webinar", "PDFs and APIs attached", "Re: IPv6 rollout", "Fix isInitialized bug",
      "Day1 kickoff, Step 1", "x86 and S3 builds", "Log4j advisory", "Win10Pro licenses", "ΔT readings", "AB12CD order", "Hello WORLD",
      "Москва: встреча в среду", "Ελληνικά νέα", "Straße und naïve café", "日本語の件名", "मित्र\u200Dता", "می\u200Cخواهم", "👩\u200D💻 update",
      "lOL", "URLs, SKUs, and MBs",
    ];
    for (const subject of ordinary) expect(tricks(subject), subject).toEqual([]);
  });

  it("judges capitals only in words of at least the minimum length", () => {
    expect(DISGUISED_WORD_MIN_LETTERS).toBe(4);
    expect(tricks("ABlC")).toEqual(["ABlC: a lowercase l in place of I"]);
    expect(tricks("AlC")).toEqual([]);
    expect(tricks("ABlCD")).toEqual(["ABlCD: a lowercase l in place of I"]);
  });
});
