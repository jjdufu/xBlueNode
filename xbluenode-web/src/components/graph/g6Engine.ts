import {
  CanvasEvent,
  ComboEvent,
  CommonEvent,
  EdgeEvent,
  ExtensionCategory,
  Graph,
  NodeEvent,
  register,
  type ComboData,
  type EdgeData,
  type GraphData,
  type GraphOptions,
  type IEvent,
  type NodeData,
  type PluginOptions,
} from "@antv/g6";
import { ReactNode } from "@antv/g6-extension-react";

let reactNodeRegistered = false;

/**
 * Idempotently registers `@antv/g6-extension-react` `ReactNode` (`type: "react-node"`)
 * inside the lazy-loaded `@antv/g6` v5 chunk so the main entry bundle stays lean.
 */
export function ensureG6ReactExtension(): void {
  if (reactNodeRegistered) return;
  register(ExtensionCategory.NODE, "react-node", ReactNode);
  reactNodeRegistered = true;
}

/**
 * Safely extracts target element `id` from a G6 v5 `IEvent` payload.
 */
export function extractG6EventTargetId(evt: IEvent): string | null {
  if (
    "target" in evt &&
    evt.target !== null &&
    typeof evt.target === "object" &&
    "id" in evt.target &&
    typeof evt.target.id === "string"
  ) {
    return evt.target.id;
  }
  return null;
}

export {
  CanvasEvent,
  ComboEvent,
  CommonEvent,
  EdgeEvent,
  Graph,
  NodeEvent,
  type ComboData,
  type EdgeData,
  type GraphData,
  type GraphOptions,
  type IEvent,
  type NodeData,
  type PluginOptions,
};
