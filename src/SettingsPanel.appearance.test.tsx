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
    emailZoom={preferences.emailZoom} onEmailZoomChange={preferences.setEmailZoom}
    emailMinimumFontSize={preferences.emailMinimumFontSize}
    onEmailMinimumFontSizeChange={preferences.setEmailMinimumFontSize}
  />;
}

describe("appearance settings", () => {
  it("saves, restores, and resets email zoom independently with guidance for all reading controls", async () => {
    const { unmount } = render(<Appearance />);
    const zoom = screen.getByRole("slider", { name: "Email Zoom" });
    const reset = screen.getByRole("button", { name: "Reset Email Zoom" });
    expect(zoom).toHaveValue("100");
    expect(reset).toBeDisabled();
    expect(zoom).toHaveAccessibleDescription(/Scales email text, images, and spacing together/);
    expect(zoom).toHaveAccessibleDescription(/turn Minimum Email Font Size off/);
    expect(zoom).toHaveAccessibleDescription(/sideways scrolling/);
    expect(screen.getByRole("combobox", { name: "Minimum Email Font Size" })).toHaveAccessibleDescription(/before Email Zoom is applied/);
    expect(screen.getByRole("slider", { name: "Font Size" })).toHaveAccessibleDescription(/Use Email Zoom/);
    fireEvent.change(zoom, { target: { value: "125" } });
    expect(zoom).toHaveAttribute("aria-valuetext", "125%");
    expect(localStorage.getItem("threestrands.settings.emailZoom")).toBe("125");
    expect(screen.getByRole("slider", { name: "Font Size" })).toHaveValue("100");
    expect(screen.getByRole("combobox", { name: "Minimum Email Font Size" })).toHaveValue("0");
    await screen.findByRole("button", { name: "Default font: System Default" });
    unmount();
    render(<Appearance />);
    expect(screen.getByRole("slider", { name: "Email Zoom" })).toHaveValue("125");
    fireEvent.click(screen.getByRole("button", { name: "Reset Email Zoom" }));
    expect(localStorage.getItem("threestrands.settings.emailZoom")).toBe("100");
  });
  it("saves, restores, and disables the minimum email font size independently of app scale", async () => {
    const { unmount } = render(<Appearance />);
    const select = screen.getByRole("combobox", { name: "Minimum Email Font Size" });
    expect(select).toHaveValue("0");
    fireEvent.change(select, { target: { value: "18" } });
    expect(select).toHaveValue("18");
    expect(screen.getByRole("slider", { name: "Font Size" })).toHaveValue("100");
    expect(localStorage.getItem("threestrands.settings.emailMinimumFontSize")).toBe("18");
    // Wait for the installed font lookup before unmounting.
    await screen.findByRole("button", { name: "Default font: System Default" });
    unmount();
    render(<Appearance />);
    const restored = screen.getByRole("combobox", { name: "Minimum Email Font Size" });
    expect(restored).toHaveValue("18");
    fireEvent.change(restored, { target: { value: "0" } });
    expect(localStorage.getItem("threestrands.settings.emailMinimumFontSize")).toBe("0");
  });
});
