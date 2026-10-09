import { useEffect, useRef, type Dispatch, type SetStateAction } from "react";
import { listenForNativeMailLinks, setMailtoHandler } from "./mailtoLink";
import type { useCorrespondence } from "./useCorrespondence";
import type { RightWorkspace } from "./useWorkspaces";

export function useNativeMailLinks(composeMailto: ReturnType<typeof useCorrespondence>["composeMailto"], setRightWorkspace: Dispatch<SetStateAction<RightWorkspace>>) {
  // mailto links — clicked in a message, or handed over by macOS when
  // ThreeStrands is the default mail app — open as a new draft here.
  const composeMailtoRef = useRef(composeMailto);
  composeMailtoRef.current = composeMailto;
  useEffect(() => {
    setMailtoHandler((request) => {
      setRightWorkspace(null);
      composeMailtoRef.current(request);
    });
    const stopListening = listenForNativeMailLinks();
    return () => {
      stopListening();
      setMailtoHandler(null);
    };
  }, [setRightWorkspace]);

}
