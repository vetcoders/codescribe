import { React } from "../lib/react.js";
import { VoiceChatLabController } from "../controllers/VoiceChatLabController.js";

const { useEffect, useMemo, useRef, useState } = React;

export function useLabController() {
  const controllerRef = useRef();
  if (!controllerRef.current) {
    controllerRef.current = new VoiceChatLabController();
  }
  const controller = controllerRef.current;
  const [state, setState] = useState(controller.getSnapshot());

  useEffect(() => {
    const off = controller.onChange((next) => setState(next));
    controller.init();
    return () => {
      off();
      controller.stopStreaming();
    };
  }, [controller]);

  return useMemo(() => ({ controller, state }), [controller, state]);
}
