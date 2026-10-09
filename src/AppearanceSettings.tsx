import { ChevronDown, Search } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { FONT_SCALE_STEP, MAX_FONT_SCALE, MIN_FONT_SCALE } from "./fontScale";
import { ACCENTS, type Accent } from "./accent";
import type { Theme } from "./theme";
import {
  DEFAULT_FONT_FAMILY,
  fontFamilyStack,
  MAX_AUTO_READ_DELAY_SECONDS,
  MIN_AUTO_READ_DELAY_SECONDS,
  type FontFamily,
} from "./settings";
import { EMAIL_MINIMUM_FONT_SIZE } from "./emailRenderingPolicy";
import { listSystemFontFamilies } from "./systemFonts";
import { ICON_SIZE } from "./iconSizes";

export function AppearanceSettings({
  theme,
  onThemeChange,
  accent,
  onAccentChange,
  fontScale,
  onFontScaleChange,
  emailMinimumFontSize,
  onEmailMinimumFontSizeChange,
  fontFamily,
  onFontFamilyChange,
}: {
  theme: Theme;
  onThemeChange(theme: Theme): void;
  accent: Accent;
  onAccentChange(accent: Accent): void;
  fontScale: number;
  onFontScaleChange(value: number): void;
  emailMinimumFontSize: number;
  onEmailMinimumFontSizeChange(value: number): void;
  fontFamily: FontFamily;
  onFontFamilyChange(value: FontFamily): void;
}) {
  const [fontFamilies, setFontFamilies] = useState<string[]>([]);
  const [fontQuery, setFontQuery] = useState("");
  const [fontsLoading, setFontsLoading] = useState(true);
  const [fontLoadFailed, setFontLoadFailed] = useState(false);
  const [fontPickerOpen, setFontPickerOpen] = useState(false);
  const fontPickerRef = useRef<HTMLDivElement>(null);
  const fontTriggerRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    let cancelled = false;
    setFontsLoading(true);
    setFontLoadFailed(false);
    listSystemFontFamilies()
      .then((families) => {
        if (!cancelled) setFontFamilies(families);
      })
      .catch(() => {
        if (!cancelled) setFontLoadFailed(true);
      })
      .finally(() => {
        if (!cancelled) setFontsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);
  useEffect(() => {
    if (!fontPickerOpen) return;
    const closeOnOutsidePointer = (event: PointerEvent) => {
      if (event.target instanceof Node && !fontPickerRef.current?.contains(event.target)) setFontPickerOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setFontPickerOpen(false);
        window.setTimeout(() => fontTriggerRef.current?.focus(), 0);
      }
    };
    document.addEventListener("pointerdown", closeOnOutsidePointer);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePointer);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [fontPickerOpen]);
  const normalizedFontQuery = fontQuery.trim().toLocaleLowerCase();
  const visibleFontFamilies = fontFamilies.filter((family) =>
    family.toLocaleLowerCase().includes(normalizedFontQuery)
  );
  const showSystemFont = !normalizedFontQuery
    || "system default".includes(normalizedFontQuery);
  const selectedFontIsInstalled = fontFamily === DEFAULT_FONT_FAMILY
    || fontFamilies.includes(fontFamily);
  const displayedFontFamilies = visibleFontFamilies.slice(0, 8);
  const hiddenFontCount = Math.max(0, visibleFontFamilies.length - displayedFontFamilies.length);
  const chooseFont = (next: FontFamily) => {
    onFontFamilyChange(next);
    setFontPickerOpen(false);
    setFontQuery("");
    window.setTimeout(() => fontTriggerRef.current?.focus(), 0);
  };
  const themeOptions: { value: Theme; label: string }[] = [
    { value: "system", label: "Match System" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
  ];
  const accentOptions: { value: Accent; label: string }[] = ACCENTS.map((value) => ({
    value,
    label: value.charAt(0).toLocaleUpperCase() + value.slice(1),
  }));
  return (
    <section className="settings-section" aria-label="Appearance">
      <h3>Theme</h3>
      <div className="settings-radio-row" role="radiogroup" aria-label="Theme">
        {themeOptions.map((option) => (
          <label key={option.value}>
            <input
              type="radio"
              name="theme"
              checked={theme === option.value}
              onChange={() => onThemeChange(option.value)}
            />
            {option.label}
          </label>
        ))}
      </div>

      <h3>Accent Color</h3>
      <p className="settings-hint">Used for highlights, selection, and buttons throughout the app.</p>
      <div className="settings-radio-row" role="radiogroup" aria-label="Accent color">
        {accentOptions.map((option) => (
          <label key={option.value}>
            <input
              type="radio"
              name="accent"
              checked={accent === option.value}
              onChange={() => onAccentChange(option.value)}
            />
            <span className="accent-swatch" data-accent={option.value} aria-hidden="true" />
            {option.label}
          </label>
        ))}
      </div>

      <h3>Font Size</h3>
      <div className="settings-row">
        <input
          type="range"
          min={MIN_FONT_SCALE}
          max={MAX_FONT_SCALE}
          step={FONT_SCALE_STEP}
          value={fontScale}
          aria-label="Font Size"
          onChange={(event) => onFontScaleChange(Number(event.target.value))}
        />
        <span>{fontScale}%</span>
      </div>

      <h3>Minimum Email Font Size</h3>
      <p className="settings-hint" id="email-minimum-font-size-hint">Enlarges small text in received emails while keeping larger text at its original size. Try 18 px for easier reading. Text inside images is unchanged.</p>
      <div className="settings-row">
        <select
          aria-label="Minimum email font size"
          aria-describedby="email-minimum-font-size-hint"
          value={emailMinimumFontSize}
          onChange={(event) => onEmailMinimumFontSizeChange(Number(event.target.value))}
        >
          <option value={EMAIL_MINIMUM_FONT_SIZE.disabled}>Off — use sender sizes</option>
          {Array.from({ length: EMAIL_MINIMUM_FONT_SIZE.maxPx - EMAIL_MINIMUM_FONT_SIZE.minPx + 1 }, (_, index) => EMAIL_MINIMUM_FONT_SIZE.minPx + index)
            .map((size) => <option key={size} value={size}>{size} px</option>)}
        </select>
      </div>

      <h3>Default Font</h3>
      <p className="settings-hint">Used throughout the app and for unformatted message text.</p>
      <div className="font-picker-control" ref={fontPickerRef}>
        <button
          type="button"
          ref={fontTriggerRef}
          className="font-picker-trigger"
          aria-label={`Default font: ${fontFamily === DEFAULT_FONT_FAMILY ? "System Default" : fontFamily}`}
          aria-haspopup="dialog"
          aria-expanded={fontPickerOpen}
          onClick={() => setFontPickerOpen((open) => !open)}
          style={{ fontFamily: fontFamilyStack(fontFamily) }}
        >
          <span>{fontFamily === DEFAULT_FONT_FAMILY ? "System Default" : fontFamily}</span>
          <span className="font-option-preview" aria-hidden="true">Aa</span>
          <ChevronDown size={ICON_SIZE.sm} aria-hidden="true" />
        </button>
        {fontPickerOpen ? (
          <div className="font-picker-popover" role="dialog" aria-label="Choose default font">
            <label className="font-search">
              <Search size={ICON_SIZE.sm} aria-hidden="true" />
              <input
                type="search"
                autoFocus
                value={fontQuery}
                placeholder="Search installed fonts"
                aria-label="Search Installed Fonts"
                onChange={(event) => setFontQuery(event.target.value)}
              />
            </label>
            <div className="font-picker" role="radiogroup" aria-label="Default font">
              {showSystemFont ? (
                <label className={`font-option${fontFamily === DEFAULT_FONT_FAMILY ? " selected" : ""}`} style={{ fontFamily: fontFamilyStack(DEFAULT_FONT_FAMILY) }}>
                  <input type="radio" name="default-font" value={DEFAULT_FONT_FAMILY} checked={fontFamily === DEFAULT_FONT_FAMILY} onChange={() => chooseFont(DEFAULT_FONT_FAMILY)} />
                  <span>System Default</span>
                  <span className="font-option-preview" aria-hidden="true">Aa</span>
                </label>
              ) : null}
              {!fontsLoading && !selectedFontIsInstalled && fontFamily !== DEFAULT_FONT_FAMILY ? (
                <label className="font-option selected" style={{ fontFamily: fontFamilyStack(fontFamily) }}>
                  <input type="radio" name="default-font" value={fontFamily} checked readOnly />
                  <span>{fontFamily} <small>Unavailable</small></span>
                  <span className="font-option-preview" aria-hidden="true">Aa</span>
                </label>
              ) : null}
              {displayedFontFamilies.map((family) => (
                <label key={family} className={`font-option${fontFamily === family ? " selected" : ""}`} style={{ fontFamily: fontFamilyStack(family) }}>
                  <input type="radio" name="default-font" value={family} checked={fontFamily === family} onChange={() => chooseFont(family)} />
                  <span>{family}</span>
                  <span className="font-option-preview" aria-hidden="true">Aa</span>
                </label>
              ))}
              {fontsLoading ? <p className="font-picker-status">Loading installed fonts…</p> : null}
              {fontLoadFailed ? <p className="font-picker-status">Installed fonts couldn’t be loaded. System default remains available.</p> : null}
              {!fontsLoading && !fontLoadFailed && !showSystemFont && visibleFontFamilies.length === 0 && normalizedFontQuery ? <p className="font-picker-status">No installed fonts match “{fontQuery.trim()}”.</p> : null}
              {!fontsLoading && hiddenFontCount > 0 ? <p className="font-picker-status">Showing 8 of {visibleFontFamilies.length}. Search to narrow the list.</p> : null}
            </div>
          </div>
        ) : null}
      </div>
    </section>
  );
}

export function ReadingSettings({
  autoReadDelaySeconds,
  onAutoReadDelayChange,
}: {
  autoReadDelaySeconds: number;
  onAutoReadDelayChange(value: number): void;
}) {
  return (
    <section className="settings-section" aria-label="Reading">
      <h3>Mark as Read</h3>
      <label className="settings-field settings-field-inline">
        <span>After Opening a Conversation</span>
        <div className="settings-row">
          <input
            type="number"
            min={MIN_AUTO_READ_DELAY_SECONDS}
            max={MAX_AUTO_READ_DELAY_SECONDS}
            step="1"
            value={autoReadDelaySeconds}
            aria-label="Auto-Read Delay"
            onChange={(event) => {
              if (event.target.value === "") return;
              const next = Number(event.target.value);
              if (Number.isFinite(next)) onAutoReadDelayChange(next);
            }}
          />
          <span>seconds</span>
        </div>
      </label>
      <p className="settings-hint">
        Set to 0 to mark conversations read immediately. The timer resets when you open a different conversation.
      </p>
    </section>
  );
}
