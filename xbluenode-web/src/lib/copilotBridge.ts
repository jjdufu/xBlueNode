import { useSyncExternalStore } from "react";
import type { Dataset } from "@/lib/api/axhub";
import type { SchemaDraft, ValidationIssue } from "@/lib/api/xblue";

export interface StudioCopilotBridgeContext {
  activeDraft: SchemaDraft | null;
  selectedNodeName: string | null;
  selectedEdgeName: string | null;
  activeInspectorTab: string;
  datasets: Dataset[];
  onEnsureDraft: () => Promise<SchemaDraft | null>;
  onDraftUpdated: (draft: SchemaDraft, notice?: string) => void;
  onDiagnosticsUpdated: (passCount: number, issues: ValidationIssue[]) => void;
  onLocateTargetPath?: (path: string) => void;
}

export interface AnalyticsCopilotBridgeContext {
  datasetNames: string[];
  applySql: (sql: string) => void;
  applyAndRunSql?: (sql: string) => void;
}

export interface CopilotBridgeSnapshot {
  open: boolean;
  studio: StudioCopilotBridgeContext | null;
  analytics: AnalyticsCopilotBridgeContext | null;
}

let snapshot: CopilotBridgeSnapshot = {
  open: false,
  studio: null,
  analytics: null,
};

const listeners = new Set<() => void>();

function notify(): void {
  for (const fn of listeners) {
    fn();
  }
}

export function setFloatingCopilotOpen(open: boolean): void {
  if (snapshot.open === open) return;
  snapshot = { ...snapshot, open };
  notify();
}

export function toggleFloatingCopilot(): void {
  snapshot = { ...snapshot, open: !snapshot.open };
  notify();
}

export function registerStudioCopilotContext(
  studio: StudioCopilotBridgeContext | null
): void {
  snapshot = { ...snapshot, studio };
  notify();
}

export function registerAnalyticsCopilotContext(
  analytics: AnalyticsCopilotBridgeContext | null
): void {
  snapshot = { ...snapshot, analytics };
  notify();
}

export function subscribeCopilotBridge(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useCopilotBridge(): CopilotBridgeSnapshot {
  return useSyncExternalStore(
    subscribeCopilotBridge,
    () => snapshot,
    () => snapshot
  );
}
