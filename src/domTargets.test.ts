import { describe, expect, it } from "vitest";
import { closestFrom, isElement } from "./domTargets";

describe("isElement", () => {
  it("accepts elements and rejects other event targets", () => {
    expect(isElement(document.createElement("div"))).toBe(true);
    expect(isElement(document.createTextNode("text"))).toBe(false);
    expect(isElement(window)).toBe(false);
    expect(isElement(null)).toBe(false);
    expect(isElement(undefined)).toBe(false);
  });

  it("accepts elements from another frame's realm", () => {
    const frame = document.createElement("iframe");
    document.body.append(frame);
    const foreign = frame.contentDocument!.createElement("a");
    expect(foreign instanceof Element).toBe(false);
    expect(isElement(foreign)).toBe(true);
    frame.remove();
  });
});

describe("closestFrom", () => {
  it("finds a matching ancestor, inclusive of the target", () => {
    const link = document.createElement("a");
    link.href = "https://example.com";
    const inner = document.createElement("span");
    link.append(inner);
    expect(closestFrom(inner, "a[href]")).toBe(link);
    expect(closestFrom(link, "a[href]")).toBe(link);
    expect(closestFrom(inner, "img")).toBeNull();
  });

  it("returns null for non-element targets", () => {
    expect(closestFrom(document.createTextNode("text"), "a")).toBeNull();
    expect(closestFrom(null, "a")).toBeNull();
  });
});
