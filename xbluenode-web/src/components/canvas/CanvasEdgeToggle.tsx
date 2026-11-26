import { ChevronsLeft, ChevronsRight } from "lucide-react";
import { cn } from "@/lib/utils";

export interface CanvasEdgeToggleProps {
  readonly side: "left" | "right";
  readonly open: boolean;
  readonly onToggle: () => void;
  readonly label: string;
  readonly collapsedLabel?: string;
  readonly topOffsetClass?: string;
  readonly className?: string;
  readonly testId?: string;
}

/**
 * Minimal double-chevron handle anchored directly onto a canvas edge (`«` / `»`),
 * preserving visual continuity across open and collapsed panel states.
 */
export function CanvasEdgeToggle({
  side,
  open,
  onToggle,
  label,
  collapsedLabel,
  topOffsetClass = "top-13",
  className,
  testId,
}: CanvasEdgeToggleProps) {
  const Icon =
    side === "right"
      ? open
        ? ChevronsRight
        : ChevronsLeft
      : open
        ? ChevronsLeft
        : ChevronsRight;

  return (
    <button
      type="button"
      data-testid={testId}
      onClick={onToggle}
      aria-label={label}
      title={label}
      className={cn(
        "absolute z-20 inline-flex items-center gap-1 border transition-all duration-150 cursor-pointer select-none",
        topOffsetClass,
        side === "right"
          ? "right-px rounded-l-md border-r-0"
          : "left-px rounded-r-md border-l-0",
        open
          ? "border-border/80 bg-card/85 hover:bg-card text-muted-foreground hover:text-foreground px-1.5 py-2 shadow-xs backdrop-blur-xs"
          : "border-border bg-card/95 hover:bg-card hover:border-xblue/60 text-foreground px-2 py-1.5 shadow-sm backdrop-blur-sm",
        className
      )}
    >
      {side === "right" && (
        <>
          <Icon className={cn("h-3.5 w-3.5 shrink-0", !open && "text-xblue")} />
          {!open && collapsedLabel && (
            <span className="text-[11px] font-medium whitespace-nowrap">
              {collapsedLabel}
            </span>
          )}
        </>
      )}
      {side === "left" && (
        <>
          {!open && collapsedLabel && (
            <span className="text-[11px] font-medium whitespace-nowrap">
              {collapsedLabel}
            </span>
          )}
          <Icon className={cn("h-3.5 w-3.5 shrink-0", !open && "text-xblue")} />
        </>
      )}
    </button>
  );
}
