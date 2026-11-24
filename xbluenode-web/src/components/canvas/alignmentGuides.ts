export interface MeasuredBox {
  readonly id: string;
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

export interface VerticalGuideLine {
  readonly x: number;
  readonly y1: number;
  readonly y2: number;
}

export interface HorizontalGuideLine {
  readonly y: number;
  readonly x1: number;
  readonly x2: number;
}

export interface AlignmentSnapResult {
  readonly snappedX: number | null;
  readonly snappedY: number | null;
  readonly verticalLines: readonly VerticalGuideLine[];
  readonly horizontalLines: readonly HorizontalGuideLine[];
}

export type AlignAction =
  | "left"
  | "centerX"
  | "right"
  | "top"
  | "centerY"
  | "distributeX"
  | "distributeY";

const EPSILON = 0.01;

/**
 * Computes 6-axis magnetic alignment snap coordinates (`left`, `centerX`, `right`,
 * `top`, `centerY`, `bottom`) and outer-envelope reference guide segments.
 */
export function computeAlignmentGuides(
  dragged: MeasuredBox,
  others: readonly MeasuredBox[],
  thresholdPx = 6
): AlignmentSnapResult {
  const draggedLeft = dragged.x;
  const draggedCenterX = dragged.x + dragged.width / 2;
  const draggedRight = dragged.x + dragged.width;

  const draggedTop = dragged.y;
  const draggedCenterY = dragged.y + dragged.height / 2;
  const draggedBottom = dragged.y + dragged.height;

  let bestAbsDx = thresholdPx + EPSILON;
  let bestDx: number | null = null;

  let bestAbsDy = thresholdPx + EPSILON;
  let bestDy: number | null = null;

  const xAxesDragged = [draggedLeft, draggedCenterX, draggedRight];
  const yAxesDragged = [draggedTop, draggedCenterY, draggedBottom];

  for (const other of others) {
    if (other.id === dragged.id) continue;

    const otherXAxes = [
      other.x,
      other.x + other.width / 2,
      other.x + other.width,
    ];
    for (const dAxis of xAxesDragged) {
      for (const oAxis of otherXAxes) {
        const dx = oAxis - dAxis;
        const absDx = Math.abs(dx);
        if (absDx <= thresholdPx && absDx < bestAbsDx - EPSILON) {
          bestAbsDx = absDx;
          bestDx = dx;
        }
      }
    }

    const otherYAxes = [
      other.y,
      other.y + other.height / 2,
      other.y + other.height,
    ];
    for (const dAxis of yAxesDragged) {
      for (const oAxis of otherYAxes) {
        const dy = oAxis - dAxis;
        const absDy = Math.abs(dy);
        if (absDy <= thresholdPx && absDy < bestAbsDy - EPSILON) {
          bestAbsDy = absDy;
          bestDy = dy;
        }
      }
    }
  }

  const snappedX = bestDx !== null ? Math.round((dragged.x + bestDx) * 100) / 100 : null;
  const snappedY = bestDy !== null ? Math.round((dragged.y + bestDy) * 100) / 100 : null;

  const effectiveX = snappedX ?? dragged.x;
  const effectiveY = snappedY ?? dragged.y;

  const effLeft = effectiveX;
  const effCenterX = effectiveX + dragged.width / 2;
  const effRight = effectiveX + dragged.width;
  const effTop = effectiveY;
  const effCenterY = effectiveY + dragged.height / 2;
  const effBottom = effectiveY + dragged.height;

  const verticalLinesMap = new Map<number, { y1: number; y2: number }>();
  if (snappedX !== null) {
    const snappedXAxes = [effLeft, effCenterX, effRight];
    for (const other of others) {
      if (other.id === dragged.id) continue;
      const otherXAxes = [
        other.x,
        other.x + other.width / 2,
        other.x + other.width,
      ];
      for (const sx of snappedXAxes) {
        for (const ox of otherXAxes) {
          if (Math.abs(sx - ox) <= 0.5) {
            const key = Math.round(ox * 10) / 10;
            const y1 = Math.min(effTop, other.y);
            const y2 = Math.max(effBottom, other.y + other.height);
            const prev = verticalLinesMap.get(key);
            if (prev) {
              verticalLinesMap.set(key, {
                y1: Math.min(prev.y1, y1),
                y2: Math.max(prev.y2, y2),
              });
            } else {
              verticalLinesMap.set(key, { y1, y2 });
            }
          }
        }
      }
    }
  }

  const horizontalLinesMap = new Map<number, { x1: number; x2: number }>();
  if (snappedY !== null) {
    const snappedYAxes = [effTop, effCenterY, effBottom];
    for (const other of others) {
      if (other.id === dragged.id) continue;
      const otherYAxes = [
        other.y,
        other.y + other.height / 2,
        other.y + other.height,
      ];
      for (const sy of snappedYAxes) {
        for (const oy of otherYAxes) {
          if (Math.abs(sy - oy) <= 0.5) {
            const key = Math.round(oy * 10) / 10;
            const x1 = Math.min(effLeft, other.x);
            const x2 = Math.max(effRight, other.x + other.width);
            const prev = horizontalLinesMap.get(key);
            if (prev) {
              horizontalLinesMap.set(key, {
                x1: Math.min(prev.x1, x1),
                x2: Math.max(prev.x2, x2),
              });
            } else {
              horizontalLinesMap.set(key, { x1, x2 });
            }
          }
        }
      }
    }
  }

  const verticalLines: VerticalGuideLine[] = Array.from(
    verticalLinesMap.entries()
  ).map(([x, seg]) => ({ x, y1: seg.y1, y2: seg.y2 }));

  const horizontalLines: HorizontalGuideLine[] = Array.from(
    horizontalLinesMap.entries()
  ).map(([y, seg]) => ({ y, x1: seg.x1, x2: seg.x2 }));

  return {
    snappedX,
    snappedY,
    verticalLines,
    horizontalLines,
  };
}

/**
 * Computes aligned or evenly distributed positions for a multi-node selection (`>= 2` nodes).
 */
export function computeSelectionAlignment(
  boxes: readonly MeasuredBox[],
  action: AlignAction
): Record<string, { x: number; y: number }> {
  const result: Record<string, { x: number; y: number }> = {};
  if (boxes.length < 2) {
    for (const b of boxes) {
      result[b.id] = { x: b.x, y: b.y };
    }
    return result;
  }

  switch (action) {
    case "left": {
      const minLeft = Math.min(...boxes.map((b) => b.x));
      for (const b of boxes) {
        result[b.id] = { x: Math.round(minLeft), y: b.y };
      }
      return result;
    }
    case "centerX": {
      const minLeft = Math.min(...boxes.map((b) => b.x));
      const maxRight = Math.max(...boxes.map((b) => b.x + b.width));
      const targetCenter = (minLeft + maxRight) / 2;
      for (const b of boxes) {
        result[b.id] = {
          x: Math.round(targetCenter - b.width / 2),
          y: b.y,
        };
      }
      return result;
    }
    case "right": {
      const maxRight = Math.max(...boxes.map((b) => b.x + b.width));
      for (const b of boxes) {
        result[b.id] = {
          x: Math.round(maxRight - b.width),
          y: b.y,
        };
      }
      return result;
    }
    case "top": {
      const minTop = Math.min(...boxes.map((b) => b.y));
      for (const b of boxes) {
        result[b.id] = { x: b.x, y: Math.round(minTop) };
      }
      return result;
    }
    case "centerY": {
      const minTop = Math.min(...boxes.map((b) => b.y));
      const maxBottom = Math.max(...boxes.map((b) => b.y + b.height));
      const targetCenter = (minTop + maxBottom) / 2;
      for (const b of boxes) {
        result[b.id] = {
          x: b.x,
          y: Math.round(targetCenter - b.height / 2),
        };
      }
      return result;
    }
    case "distributeX": {
      const sorted = [...boxes].sort((a, b) => a.x - b.x);
      const first = sorted[0];
      const last = sorted[sorted.length - 1];
      if (!first || !last) return result;
      const minLeft = first.x;
      const maxRight = Math.max(...sorted.map((b) => b.x + b.width));
      const totalWidth = sorted.reduce((acc, b) => acc + b.width, 0);
      const gap = (maxRight - minLeft - totalWidth) / (sorted.length - 1);
      let cursor = minLeft;
      for (const b of sorted) {
        result[b.id] = { x: Math.round(cursor), y: b.y };
        cursor += b.width + gap;
      }
      return result;
    }
    case "distributeY": {
      const sorted = [...boxes].sort((a, b) => a.y - b.y);
      const first = sorted[0];
      const last = sorted[sorted.length - 1];
      if (!first || !last) return result;
      const minTop = first.y;
      const maxBottom = Math.max(...sorted.map((b) => b.y + b.height));
      const totalHeight = sorted.reduce((acc, b) => acc + b.height, 0);
      const gap = (maxBottom - minTop - totalHeight) / (sorted.length - 1);
      let cursor = minTop;
      for (const b of sorted) {
        result[b.id] = { x: b.x, y: Math.round(cursor) };
        cursor += b.height + gap;
      }
      return result;
    }
  }
}
