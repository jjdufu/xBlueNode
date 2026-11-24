import { useCallback, useEffect, useRef } from "react";

export function easeOutCubic(t: number): number {
  const clamped = Math.max(0, Math.min(1, t));
  return 1 - Math.pow(1 - clamped, 3);
}

/**
 * Smoothly interpolates node coordinates over `durationMs` (default 240ms) using
 * `easeOutCubic` + `requestAnimationFrame` when layout or tier folding triggers.
 */
export function useAnimatedNodes(durationMs = 240) {
  const rafRef = useRef<number | null>(null);

  const cancelAnimation = useCallback(() => {
    if (rafRef.current !== null && typeof window !== "undefined") {
      window.cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
  }, []);

  useEffect(() => cancelAnimation, [cancelAnimation]);

  const animatePositions = useCallback(
    (
      fromPositions: Readonly<Record<string, { x: number; y: number }>>,
      toPositions: Readonly<Record<string, { x: number; y: number }>>,
      onFrame: (current: Record<string, { x: number; y: number }>) => void,
      onComplete?: () => void
    ) => {
      cancelAnimation();

      if (
        typeof window === "undefined" ||
        typeof window.requestAnimationFrame !== "function" ||
        durationMs <= 0
      ) {
        onFrame({ ...toPositions });
        onComplete?.();
        return;
      }

      const startTime = performance.now();
      const keys = Object.keys(toPositions);

      const step = (now: number) => {
        const elapsed = now - startTime;
        const progress = Math.min(1, elapsed / durationMs);
        const eased = easeOutCubic(progress);

        const interpolated: Record<string, { x: number; y: number }> = {};
        for (const id of keys) {
          const target = toPositions[id];
          if (!target) continue;
          const origin = fromPositions[id] ?? target;
          interpolated[id] = {
            x: origin.x + (target.x - origin.x) * eased,
            y: origin.y + (target.y - origin.y) * eased,
          };
        }

        onFrame(interpolated);

        if (progress < 1) {
          rafRef.current = window.requestAnimationFrame(step);
        } else {
          rafRef.current = null;
          onComplete?.();
        }
      };

      rafRef.current = window.requestAnimationFrame(step);
    },
    [cancelAnimation, durationMs]
  );

  return { animatePositions, cancelAnimation };
}
