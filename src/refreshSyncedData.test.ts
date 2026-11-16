import { afterEach, describe, expect, it, vi } from "vitest";
import { replicatedSyncEnabled, replicatedSyncNow } from "./replicatedSync";
import { refreshSyncedData } from "./refreshSyncedData";
vi.mock("./replicatedSync", () => ({ replicatedSyncEnabled: vi.fn(), replicatedSyncNow: vi.fn() }));
afterEach(() => vi.resetAllMocks());
describe("manual device refresh", () => {
  it.each([true, false])("checks peers only when enabled (%s)", async (enabled) => {
    vi.mocked(replicatedSyncEnabled).mockResolvedValue(enabled);
    await refreshSyncedData();
    expect(replicatedSyncNow).toHaveBeenCalledTimes(enabled ? 1 : 0);
  });
  it("reports transport failures", async () => {
    vi.mocked(replicatedSyncEnabled).mockResolvedValue(true);
    vi.mocked(replicatedSyncNow).mockRejectedValue(new Error("Transport unavailable"));
    await expect(refreshSyncedData()).rejects.toThrow("Transport unavailable");
  });
});
