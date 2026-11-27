import { createContext, useContext } from "react";
import { UNSUPPORTED_RECORDING, type RecordingController } from "./useMeetingRecording";

/** Provided by App; without it (a component tested alone) nothing can be recorded. */
export const RecordingContext = createContext<RecordingController>(UNSUPPORTED_RECORDING);

export function useRecording(): RecordingController {
  return useContext(RecordingContext);
}
