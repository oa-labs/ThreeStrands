import { act, cleanup, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { readFileSync } from "node:fs";
import postcss from "postcss";
import { afterEach, describe, expect, it, vi } from "vitest";
import { isDaytime, readTodayBackdrop, skyPhase, sunHeight, TODAY_BACKDROP_KEY, useTodayBackdrop, writeTodayBackdrop } from "./backdropPreference";
import { TodayBackdropSettings } from "./AppearanceSettings";
import { readExportablePreferences } from "./userPreferences";
import { TodayBackdrop } from "./TodayBackdrop";
import { SettingsHarness } from "./test/settingsHarness";

vi.mock("./systemFonts", () => ({ listSystemFontFamilies: vi.fn().mockResolvedValue([]) }));

afterEach(() => { cleanup(); localStorage.clear(); vi.restoreAllMocks(); });

const at = (hours: number, minutes = 0) => new Date(2026, 10, 17, hours, minutes);

describe("Today Backdrop preference", () => {
  it("is off by default, persists on this device, and reads as off when storage fails", () => {
    expect(readTodayBackdrop()).toBe(false);
    writeTodayBackdrop(true);
    expect(localStorage.getItem(TODAY_BACKDROP_KEY)).toBe("on");
    expect(readTodayBackdrop()).toBe(true);
    writeTodayBackdrop(false);
    expect(readTodayBackdrop()).toBe(false);
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
    expect(readTodayBackdrop()).toBe(false);
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    expect(() => writeTodayBackdrop(true)).not.toThrow();
  });

  it("updates every reader when Settings changes it", () => {
    const { result } = renderHook(() => useTodayBackdrop());
    expect(result.current).toBe(false);
    act(() => writeTodayBackdrop(true));
    expect(result.current).toBe(true);
  });

  it("is not a synced or exported preference: the settings export neither reads nor carries it, and the transfer code never names it", () => {
    const before = JSON.stringify(readExportablePreferences());
    writeTodayBackdrop(true);
    expect(JSON.stringify(readExportablePreferences())).toBe(before);
    expect(Object.keys(readExportablePreferences()).filter((key) => /backdrop/i.test(key))).toEqual([]);
    for (const file of ["src-tauri/src/transfer.rs", "src/userPreferences.ts", "src/DataTransferSettings.tsx"]) {
      expect(readFileSync(file, "utf8").toLowerCase()).not.toContain("backdrop");
    }
  });

  it("is chosen under Settings → Appearance → Today Backdrop, with its help attached", () => {
    render(<SettingsHarness initialSection="appearance" />);
    const box = screen.getByRole("checkbox", { name: "Show a Day and Evening Backdrop" });
    expect(box).not.toBeChecked();
    expect(box).toHaveAccessibleDescription();
    fireEvent.click(box);
    expect(box).toBeChecked();
    expect(localStorage.getItem(TODAY_BACKDROP_KEY)).toBe("on");
    cleanup();
    render(<TodayBackdropSettings />);
    expect(screen.getByRole("checkbox", { name: "Show a Day and Evening Backdrop" })).toBeChecked();
    fireEvent.click(screen.getByRole("checkbox", { name: "Show a Day and Evening Backdrop" }));
    expect(readTodayBackdrop()).toBe(false);
  });
});

describe("sunHeight", () => {
  it("rises from the horizon at 6 am to its peak at noon and sets at 6 pm, staying down at night", () => {
    expect(sunHeight(at(3))).toBe(0);
    expect(sunHeight(at(6))).toBe(0);
    expect(sunHeight(at(12))).toBeCloseTo(1, 5);
    expect(sunHeight(at(18))).toBe(0);
    expect(sunHeight(at(23, 59))).toBe(0);
    expect(sunHeight(at(9))).toBeGreaterThan(sunHeight(at(7)));
    expect(sunHeight(at(9))).toBeLessThan(sunHeight(at(11)));
    expect(sunHeight(at(15))).toBeCloseTo(sunHeight(at(9)), 5);
  });
});

describe("isDaytime", () => {
  it("is day from sunrise up to sunset and night otherwise", () => {
    expect(isDaytime(at(5, 59))).toBe(false);
    expect(isDaytime(at(6))).toBe(true);
    expect(isDaytime(at(12))).toBe(true);
    expect(isDaytime(at(17, 59))).toBe(true);
    expect(isDaytime(at(18))).toBe(false);
    expect(isDaytime(at(22, 30))).toBe(false);
    expect(isDaytime(at(0))).toBe(false);
  });
});

describe("skyPhase", () => {
  it("is night from 9 pm to 4:59 am, dusk from 5 to 5:59 am and 6 to 8:59 pm, and day between", () => {
    const phases = [[4, 59, "night"], [5, 0, "dusk"], [5, 59, "dusk"], [6, 0, "day"], [17, 59, "day"], [18, 0, "dusk"], [20, 59, "dusk"], [21, 0, "night"], [0, 0, "night"]] as const;
    for (const [hours, minutes, phase] of phases) expect(skyPhase(at(hours, minutes)), `${hours}:${minutes}`).toBe(phase);
    expect(isDaytime(at(20, 59))).toBe(false);
  });
});

describe("TodayBackdrop", () => {
  const scene = () => document.querySelector(".today-backdrop") as HTMLElement;

  it("draws a day sky whose sun is higher at noon than in the morning, hidden from assistive technology", () => {
    const { rerender } = render(<TodayBackdrop now={at(8)} />);
    expect(scene()).toHaveAttribute("aria-hidden", "true");
    expect(scene()).toHaveAttribute("data-phase", "day");
    const sunY = () => Number(scene().querySelector(".today-backdrop-sun")!.getAttribute("cy"));
    const morning = sunY();
    rerender(<TodayBackdrop now={at(12)} />);
    // Smaller y is higher in the picture.
    expect(sunY()).toBeLessThan(morning);
    expect(scene().querySelector(".today-backdrop-moon")).toBeNull();
    expect(scene().querySelectorAll(".today-backdrop-star")).toHaveLength(0);
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("draws a dusk sky with a moon and a few stars after sunset", () => {
    render(<TodayBackdrop now={at(19, 30)} />);
    expect(scene()).toHaveAttribute("data-phase", "dusk");
    expect(scene()).toHaveAttribute("aria-hidden", "true");
    expect(scene().querySelector(".today-backdrop-moon")).not.toBeNull();
    expect(scene().querySelectorAll(".today-backdrop-star").length).toBeGreaterThanOrEqual(3);
    expect(scene().querySelector(".today-backdrop-sun")).toBeNull();
    expect(scene().querySelector("[focusable='true'], a, button, input")).toBeNull();
  });

  it("draws a late-night sky with more stars than dusk, a moon, and no sun", () => {
    const { rerender } = render(<TodayBackdrop now={at(19, 30)} />);
    const duskStars = scene().querySelectorAll(".today-backdrop-star").length;
    rerender(<TodayBackdrop now={at(0)} />);
    expect(scene()).toHaveAttribute("data-phase", "night");
    expect(scene()).toHaveAttribute("aria-hidden", "true");
    expect(scene().querySelectorAll(".today-backdrop-star").length).toBeGreaterThan(duskStars);
    expect(scene().querySelector(".today-backdrop-moon")).not.toBeNull();
    expect(scene().querySelector(".today-backdrop-sun")).toBeNull();
    // Fixed places: the same phase draws the same stars.
    const positions = () => [...scene().querySelectorAll(".today-backdrop-star")].map((star) => `${star.getAttribute("cx")},${star.getAttribute("cy")}`);
    const first = positions();
    rerender(<TodayBackdrop now={at(2)} />);
    expect(positions()).toEqual(first);
  });

  it("keeps the moon round and fully inset in narrow, wide, and wrapped headers", () => {
    const { rerender } = render(<TodayBackdrop now={at(22)} />);
    const css = postcss.parse(readFileSync("src/styles.css", "utf8"));
    const tokens = new Map<string, string>();
    css.walkRules(":root", (rule) => rule.walkDecls(/^--/, (declaration) => { tokens.set(declaration.prop, declaration.value); }));
    const geometry = new Map<string, string>();
    css.walkRules(".today-backdrop .today-backdrop-moon-scene", (rule) => rule.walkDecls((declaration) => { geometry.set(declaration.prop, declaration.value); }));
    expect(geometry.get("position")).toBe("absolute");
    expect(geometry.get("top")).toBe("var(--space-3)");
    expect(geometry.get("right")).toBe("12%");
    expect(geometry.get("width")).toBe("var(--today-backdrop-moon-size)");
    expect(geometry.get("height")).toBe("calc(100% - 2 * var(--space-3))");
    const inset = Number.parseFloat(tokens.get("--space-3")!);
    const width = Number.parseFloat(tokens.get("--today-backdrop-moon-size")!);
    for (const time of [at(19), at(22)]) {
      rerender(<TodayBackdrop now={time} />);
      const moon = scene().querySelector(".today-backdrop-moon")!;
      const viewport = moon.closest("svg")!;
      expect(viewport).toHaveClass("today-backdrop-moon-scene");
      expect(viewport).toHaveAttribute("preserveAspectRatio", "xMidYMid meet");
      expect(viewport).toHaveAttribute("focusable", "false");
      const [, , sceneWidth, sceneHeight] = viewport.getAttribute("viewBox")!.split(" ").map(Number);
      const radius = Number(moon.getAttribute("r"));
      const cx = Number(moon.getAttribute("cx"));
      const cy = Number(moon.getAttribute("cy"));
      for (const [headerWidth, headerHeight] of [[360, 80], [1500, 88], [3440, 88], [5120, 88], [360, 160]]) {
        // SVG meet preserves circles and letterboxes within the inset viewport.
        const height = headerHeight - 2 * inset;
        const scale = Math.min(width / sceneWidth, height / sceneHeight);
        const left = headerWidth * (1 - 0.12) - width + (width - sceneWidth * scale) / 2;
        const top = inset + (height - sceneHeight * scale) / 2;
        expect(top + (cy - radius) * scale).toBeGreaterThanOrEqual(inset);
        expect(top + (cy + radius) * scale).toBeLessThanOrEqual(headerHeight - inset);
        expect(left + (cx - radius) * scale).toBeGreaterThan(0);
        expect(left + (cx + radius) * scale).toBeLessThan(headerWidth);
      }
    }
  });

  it("follows the clock: night, then dusk, day, dusk, and night again", () => {
    const { rerender } = render(<TodayBackdrop now={at(4, 59)} />);
    const phases: Array<[Date, string]> = [[at(4, 59), "night"], [at(5), "dusk"], [at(6), "day"], [at(18), "dusk"], [at(21), "night"]];
    for (const [time, phase] of phases) {
      rerender(<TodayBackdrop now={time} />);
      expect(scene()).toHaveAttribute("data-phase", phase);
    }
    expect(scene().querySelector(".today-backdrop-sun")).toBeNull();
  });

  it("keeps art in the header, tints the gutters behind opaque panels, and respects pointer and motion preferences", () => {
    const css = postcss.parse(readFileSync("src/styles.css", "utf8"));
    const rule = (selector: string, atRule?: string) => {
      let found: postcss.Rule | undefined;
      css.walkRules(selector, (candidate) => {
        const parent = candidate.parent;
        if (atRule ? parent?.type === "atrule" && (parent as postcss.AtRule).params.includes(atRule) : parent?.type === "root") found = candidate;
      });
      return found;
    };
    const value = (candidate: postcss.Rule | undefined, prop: string) => candidate?.nodes.find((node): node is postcss.Declaration => node.type === "decl" && node.prop === prop)?.value;
    const base = rule(".today-backdrop");
    expect(value(base, "position")).toBe("absolute");
    expect(value(base, "pointer-events")).toBe("none");
    expect(value(base, "z-index")).toBe("0");
    expect(value(base, "background")).toBe("linear-gradient(to bottom, var(--today-sky-top), var(--today-sky-bottom))");
    // The header keeps its sky colors; the night surround has a lighter blue of its own.
    for (const [phase, token] of [["day", "sky"], ["dusk", "dusk"], ["night", "night"]] as const) {
      const selector = phase === "day" ? ".today-workspace[data-backdrop], .today-backdrop"
        : `.today-workspace[data-backdrop="${phase}"], .today-backdrop[data-phase="${phase}"]`;
      expect(value(rule(selector), "--today-sky-top")).toBe(`var(--backdrop-${token}-top)`);
      expect(value(rule(selector), "--today-sky-bottom")).toBe(`var(--backdrop-${token}-bottom)`);
      expect(value(rule(selector), "--today-gutter-tint")).toBe(phase === "dusk" ? "25%" : "65%");
    }
    expect(value(rule('.today-workspace[data-backdrop="night"], .today-backdrop[data-phase="night"]'), "--today-gutter-color")).toBe("var(--backdrop-night-surround)");
    expect(value(rule(".today-workspace[data-backdrop], .today-backdrop"), "--today-gutter-color")).toBe("var(--today-sky-top)");
    expect(value(rule(".today-workspace[data-backdrop]"), "background")).toBe("linear-gradient(to bottom, color-mix(in srgb, var(--today-gutter-color) var(--today-gutter-tint), var(--background)), var(--background) 70%)");
    expect(value(rule(".today-workspace"), "background")).toBe("var(--background)");
    expect(value(rule(".today-work, .today-time"), "background")).toBe("var(--surface)");
    // The header's own content paints above the art.
    expect(value(rule(".today-header > :not(.today-backdrop)"), "position")).toBe("relative");
    expect(value(rule(".today-header"), "position")).toBe("relative");
    expect(value(rule(".today-header"), "isolation")).toBe("isolate");
    expect(value(base, "animation")).toMatch(/today-backdrop-in var\(--duration-slow\)/);
    // The one motion is that fade, which the global reduced-motion override collapses (and motion.test.ts holds in place).
    let reduced: postcss.AtRule | undefined;
    css.walkAtRules("media", (at) => { if (at.params.includes("prefers-reduced-motion: reduce")) reduced = at; });
    const overridden = new Map<string, postcss.Declaration>();
    reduced!.walkDecls((declaration) => { overridden.set(declaration.prop, declaration); });
    expect(overridden.get("animation-duration")).toMatchObject({ value: "0.01ms", important: true });
    expect(overridden.get("animation-iteration-count")).toMatchObject({ value: "1", important: true });
  });
});
