import { useCallback, useEffect, useRef, useState } from "react";
import { temporal } from "zundo";
import { createStore } from "zustand/vanilla";

export interface CanvasHistorySlice<TDoc> {
  readonly doc: TDoc;
  readonly setDoc: (next: TDoc) => void;
}

export interface CanvasHistoryController<TDoc> {
  readonly getDoc: () => TDoc;
  readonly pushSnapshot: (next: TDoc) => void;
  readonly resetHistory: (seed: TDoc) => void;
  readonly undo: () => TDoc | null;
  readonly redo: () => TDoc | null;
  readonly canUndo: () => boolean;
  readonly canRedo: () => boolean;
  readonly pastCount: () => number;
  readonly futureCount: () => number;
  readonly subscribe: (listener: () => void) => () => void;
}

/**
 * Pure `zustand` + `zundo` temporal history store capped at `limit` (default 50) steps.
 * Usable both inside React hooks and directly inside deterministic Node unit tests.
 */
export function createCanvasHistoryStore<TDoc>(
  initialDoc: TDoc,
  limit = 50
): CanvasHistoryController<TDoc> {
  const store = createStore<CanvasHistorySlice<TDoc>>()(
    temporal(
      (set) => ({
        doc: initialDoc,
        setDoc: (next: TDoc) => set({ doc: next }),
      }),
      {
        limit,
        partialize: (state) => ({ doc: state.doc }),
        equality: (a, b) => Object.is(a.doc, b.doc),
      }
    )
  );

  return {
    getDoc: () => store.getState().doc,
    pushSnapshot: (next: TDoc) => {
      store.getState().setDoc(next);
    },
    resetHistory: (seed: TDoc) => {
      store.temporal.getState().pause();
      store.setState({ doc: seed });
      store.temporal.getState().clear();
      store.temporal.getState().resume();
    },
    undo: () => {
      const temporalState = store.temporal.getState();
      if (temporalState.pastStates.length === 0) {
        return null;
      }
      temporalState.undo(1);
      return store.getState().doc;
    },
    redo: () => {
      const temporalState = store.temporal.getState();
      if (temporalState.futureStates.length === 0) {
        return null;
      }
      temporalState.redo(1);
      return store.getState().doc;
    },
    canUndo: () => store.temporal.getState().pastStates.length > 0,
    canRedo: () => store.temporal.getState().futureStates.length > 0,
    pastCount: () => store.temporal.getState().pastStates.length,
    futureCount: () => store.temporal.getState().futureStates.length,
    subscribe: (listener: () => void) => {
      const unsubMain = store.subscribe(listener);
      const unsubTemp = store.temporal.subscribe(listener);
      return () => {
        unsubMain();
        unsubTemp();
      };
    },
  };
}

function isEditableKeyboardTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") {
    return true;
  }
  return Boolean(target.isContentEditable || target.closest(".monaco-editor"));
}

export interface UseCanvasHistoryOptions<TDoc> {
  readonly currentDoc: TDoc;
  readonly onApplySnapshot: (doc: TDoc) => void;
  readonly enabled?: boolean;
  readonly limit?: number;
}

/**
 * React binding for `zundo` 50-step undo/redo with `⌘Z` / `⌘⇧Z` / `Ctrl+Z` / `Ctrl+Shift+Z` / `Ctrl+Y`.
 */
export function useCanvasHistory<TDoc>({
  currentDoc,
  onApplySnapshot,
  enabled = true,
  limit = 50,
}: UseCanvasHistoryOptions<TDoc>) {
  const controllerRef = useRef<CanvasHistoryController<TDoc> | null>(null);
  if (controllerRef.current === null) {
    controllerRef.current = createCanvasHistoryStore(currentDoc, limit);
  }
  const controller = controllerRef.current;

  const [, setTick] = useState(0);
  const applyingRef = useRef(false);

  useEffect(() => {
    return controller.subscribe(() => {
      setTick((v) => v + 1);
    });
  }, [controller]);

  const recordDoc = useCallback(
    (nextDoc: TDoc) => {
      if (applyingRef.current) return;
      controller.pushSnapshot(nextDoc);
    },
    [controller]
  );

  const resetHistory = useCallback(
    (seedDoc: TDoc) => {
      controller.resetHistory(seedDoc);
    },
    [controller]
  );

  const undo = useCallback(() => {
    if (!enabled) return false;
    applyingRef.current = true;
    try {
      const prev = controller.undo();
      if (prev !== null) {
        onApplySnapshot(prev);
        return true;
      }
      return false;
    } finally {
      applyingRef.current = false;
    }
  }, [controller, enabled, onApplySnapshot]);

  const redo = useCallback(() => {
    if (!enabled) return false;
    applyingRef.current = true;
    try {
      const next = controller.redo();
      if (next !== null) {
        onApplySnapshot(next);
        return true;
      }
      return false;
    } finally {
      applyingRef.current = false;
    }
  }, [controller, enabled, onApplySnapshot]);

  useEffect(() => {
    if (!enabled || typeof window === "undefined") return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (isEditableKeyboardTarget(event.target)) return;
      const mod = event.metaKey || event.ctrlKey;
      if (!mod) return;
      const key = event.key.toLowerCase();
      if (key === "z" && !event.shiftKey) {
        if (controller.canUndo()) {
          event.preventDefault();
          undo();
        }
      } else if ((key === "z" && event.shiftKey) || (key === "y" && !event.metaKey)) {
        if (controller.canRedo()) {
          event.preventDefault();
          redo();
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [controller, enabled, redo, undo]);

  return {
    undo,
    redo,
    recordDoc,
    resetHistory,
    canUndo: controller.canUndo(),
    canRedo: controller.canRedo(),
    pastCount: controller.pastCount(),
    futureCount: controller.futureCount(),
  };
}
