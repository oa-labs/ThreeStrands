import { vi } from "vitest";
import { IDLE_RECORDING_STATUS, UNSUPPORTED_RECORDING, type RecordingController } from "../useMeetingRecording";

/** A recording controller with spies, for rendering a component that reads the recording context. */
export const recordingController = (overrides: Partial<RecordingController> = {}): RecordingController => ({
  ...UNSUPPORTED_RECORDING,
  capabilities: { supported: true, systemAudio: { kind: "available" } },
  supported: true,
  enabled: true,
  status: IDLE_RECORDING_STATUS,
  start: vi.fn().mockResolvedValue(undefined),
  startAdHoc: vi.fn().mockResolvedValue(undefined),
  stop: vi.fn().mockResolvedValue(undefined),
  pause: vi.fn().mockResolvedValue(undefined),
  resume: vi.fn().mockResolvedValue(undefined),
  ...overrides,
});
