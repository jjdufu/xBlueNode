import {
  EventStreamContentType,
  fetchEventSource,
} from "@microsoft/fetch-event-source";
import { getAccessToken } from "./api/auth";

export interface ViewInvalidateEvent {
  widget_ids: string[];
  dashboard_ids?: string[];
}

export interface SubscribeDashboardSseOptions {
  dashboardId: string;
  onInvalidate: (payload: ViewInvalidateEvent) => void;
  onAlert?: (payload: unknown) => void;
  onReconnected?: () => void;
  onError?: (err: unknown) => void;
}

class FatalSseError extends Error {}

/**
 * Connects to `GET /api/xblueview/dashboards/{id}/stream` using `@microsoft/fetch-event-source`
 * so `Authorization: Bearer <token>` headers are preserved, with exponential backoff reconnect.
 */
export function subscribeDashboardSse(
  options: SubscribeDashboardSseOptions
): () => void {
  const controller = new AbortController();
  let stopped = false;
  let retryDelayMs = 1000;
  let hadDisconnect = false;

  const waitBackoff = async () => {
    await new Promise<void>((resolve) => {
      const timer = setTimeout(resolve, retryDelayMs);
      controller.signal.addEventListener(
        "abort",
        () => {
          clearTimeout(timer);
          resolve();
        },
        { once: true }
      );
    });
    retryDelayMs = Math.min(retryDelayMs * 2, 30000);
  };

  const connect = async () => {
    while (!stopped && !controller.signal.aborted) {
      const token = getAccessToken();
      const headers: Record<string, string> = {
        Accept: "text/event-stream",
      };
      if (token) {
        headers.Authorization = `Bearer ${token}`;
      }

      try {
        await fetchEventSource(
          `/api/xblueview/dashboards/${options.dashboardId}/stream`,
          {
            method: "GET",
            headers,
            signal: controller.signal,
            openWhenHidden: true,
            async onopen(response) {
              if (
                response.ok &&
                response.headers
                  .get("content-type")
                  ?.includes(EventStreamContentType)
              ) {
                retryDelayMs = 1000;
                if (hadDisconnect) {
                  hadDisconnect = false;
                  options.onReconnected?.();
                }
                return;
              }
              if (response.status >= 400 && response.status < 500) {
                throw new FatalSseError(`SSE rejected (${response.status})`);
              }
              throw new Error(`SSE HTTP ${response.status}`);
            },
            onmessage(ev) {
              if (!ev.data) return;
              try {
                const parsed: unknown = JSON.parse(ev.data);
                if (ev.event === "invalidate") {
                  const obj = parsed as Partial<ViewInvalidateEvent>;
                  options.onInvalidate({
                    widget_ids: Array.isArray(obj.widget_ids)
                      ? obj.widget_ids
                      : [],
                    dashboard_ids: Array.isArray(obj.dashboard_ids)
                      ? obj.dashboard_ids
                      : undefined,
                  });
                } else if (ev.event === "alert") {
                  options.onAlert?.(parsed);
                }
              } catch {
                // Ignore malformed JSON keepalive or partial frames.
              }
            },
            onerror(err) {
              if (err instanceof FatalSseError) {
                throw err;
              }
              hadDisconnect = true;
              options.onError?.(err);
              throw err;
            },
          }
        );

        if (stopped || controller.signal.aborted) {
          break;
        }
        hadDisconnect = true;
        options.onError?.(new Error("SSE stream closed (EOF)"));
        await waitBackoff();
      } catch (err) {
        if (stopped || controller.signal.aborted || err instanceof FatalSseError) {
          break;
        }
        hadDisconnect = true;
        options.onError?.(err);
        await waitBackoff();
      }
    }
  };

  void connect();

  return () => {
    stopped = true;
    controller.abort();
  };
}
