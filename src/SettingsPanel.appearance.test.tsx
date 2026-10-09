import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppearanceSettings } from "./AppearanceSettings";
import { useAppPreferences } from "./useAppPreferences";

vi.mock("./systemFonts", () => ({ listSystemFontFamilies: vi.fn().mockResolvedValue([]) }));

afterEach(() => { cleanup(); localStorage.clear(); });

function Appearance() {
  const preferences = useAppPreferences();
  return <AppearanceSettings
    theme={preferences.theme} onThemeChange={preferences.setTheme}
    accent={preferences.accent} onAccentChange={preferences.setAccent}
    fontScale={preferences.fontScale} onFontScaleChange={preferences.setFontScale}
    fontFamily={preferences.fontFamily} onFontFamilyChange={preferences.setFontFamily}
    emailMinimumFontSize={preferences.emailMinimumFontSize}
    onEmailMinimumFontSizeChange={preferences.setEmailMinimumFontSize}
  />;
}

describe("appearance settings", () => {
  it("saves, restores, and disables the minimum email font size independently of app scale", async () => {
    const { unmount } = render(<Appearance />);
    const select = screen.getByRole("combobox", { name: "Minimum email font size" });
    expect(select).toHaveValue("0");
    fireEvent.change(select, { target: { value: "18" } });
    expect(select).toHaveValue("18");
    expect(screen.getByRole("slider", { name: "Font Size" })).toHaveValue("100");
    expect(localStorage.getItem("threestrands.settings.emailMinimumFontSize")).toBe("18");
    // Wait for the installed font lookup before unmounting.
    await screen.findByRole("button", { name: "Default font: System Default" });
    unmount();
    render(<Appearance />);
    const restored = screen.getByRole("combobox", { name: "Minimum email font size" });
    expect(restored).toHaveValue("18");
    fireEvent.change(restored, { target: { value: "0" } });
    expect(localStorage.getItem("threestrands.settings.emailMinimumFontSize")).toBe("0");
  });
});
