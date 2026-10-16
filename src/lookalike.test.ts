import { describe, expect, it } from "vitest";
import { knownSites, LOOKALIKE_MIN_TYPO_LENGTH, lookalikeOf, mixesScripts, skeleton, spelledOutSite, unicodeHost } from "./lookalike";

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
