import { useEffect, useRef, useState } from "react";

export type DaemonState = "connected" | "reconnecting" | "offline";

/** Live daemon reachability from the gateway's authenticated event stream. */
export function useDaemonConnection(): DaemonState {
  const [state, setState] = useState<DaemonState>("reconnecting");
  useEffect(() => {
    const events = new EventSource("/api/v1/events");
    const update = (event: Event) => {
      try {
        setState(JSON.parse((event as MessageEvent).data).connected ? "connected" : "offline");
      } catch {
        setState("reconnecting");
      }
    };
    events.addEventListener("daemon", update);
    events.onerror = () => setState("reconnecting");
    return () => events.close();
  }, []);
  return state;
}

/** Closes a popover when focus or a pointer leaves it, or on Escape. */
export function useDismiss<T extends HTMLElement>(open: boolean, close: () => void) {
  const ref = useRef<T>(null);
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) close();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        close();
      }
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown, true);
    };
  }, [open, close]);
  return ref;
}

export function isTyping(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement ||
    (target instanceof HTMLElement && target.isContentEditable)
  );
}

/** Warns before a reload would discard unsaved editor content. */
export function useUnsavedGuard(dirty: boolean) {
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);
}
