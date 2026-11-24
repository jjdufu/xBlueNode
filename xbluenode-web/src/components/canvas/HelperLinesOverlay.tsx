import { useViewport } from "@xyflow/react";
import type {
  HorizontalGuideLine,
  VerticalGuideLine,
} from "./alignmentGuides.ts";

export interface HelperLinesOverlayProps {
  verticalLines: readonly VerticalGuideLine[];
  horizontalLines: readonly HorizontalGuideLine[];
}

/**
 * Renders 1px dashed `--color-xblue` magnetic alignment guides over a React Flow canvas.
 */
export function HelperLinesOverlay({
  verticalLines,
  horizontalLines,
}: HelperLinesOverlayProps) {
  const { x: vpX, y: vpY, zoom } = useViewport();

  if (verticalLines.length === 0 && horizontalLines.length === 0) {
    return null;
  }

  return (
    <svg
      data-testid="canvas-helper-lines"
      className="pointer-events-none absolute inset-0 z-20 h-full w-full overflow-visible"
    >
      {verticalLines.map((line, idx) => {
        const sx = line.x * zoom + vpX;
        const sy1 = line.y1 * zoom + vpY;
        const sy2 = line.y2 * zoom + vpY;
        return (
          <line
            key={`v-${idx}-${line.x}`}
            data-testid="helper-line-vertical"
            x1={sx}
            y1={sy1}
            x2={sx}
            y2={sy2}
            stroke="var(--color-xblue)"
            strokeWidth={1}
            strokeDasharray="4 3"
          />
        );
      })}
      {horizontalLines.map((line, idx) => {
        const sy = line.y * zoom + vpY;
        const sx1 = line.x1 * zoom + vpX;
        const sx2 = line.x2 * zoom + vpX;
        return (
          <line
            key={`h-${idx}-${line.y}`}
            data-testid="helper-line-horizontal"
            x1={sx1}
            y1={sy}
            x2={sx2}
            y2={sy}
            stroke="var(--color-xblue)"
            strokeWidth={1}
            strokeDasharray="4 3"
          />
        );
      })}
    </svg>
  );
}
