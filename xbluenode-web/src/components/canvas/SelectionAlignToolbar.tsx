import {
  AlignCenterHorizontal,
  AlignCenterVertical,
  AlignEndHorizontal,
  AlignHorizontalSpaceAround,
  AlignStartHorizontal,
  AlignStartVertical,
  AlignVerticalSpaceAround,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { IconButton } from "@/components/ui/primitives";
import type { AlignAction } from "./alignmentGuides.ts";

export interface SelectionAlignToolbarProps {
  selectedCount: number;
  onAlign: (action: AlignAction) => void;
}

/**
 * Floating multi-selection alignment & even-distribution bar (`>= 2` selected nodes).
 */
export function SelectionAlignToolbar({
  selectedCount,
  onAlign,
}: SelectionAlignToolbarProps) {
  const { tx } = useI18n();

  if (selectedCount < 2) {
    return null;
  }

  const actions: ReadonlyArray<{
    action: AlignAction;
    zh: string;
    en: string;
    icon: React.ReactNode;
    minCount: number;
  }> = [
    {
      action: "left",
      zh: "左对齐",
      en: "Align Left",
      icon: <AlignStartVertical className="h-3.5 w-3.5" />,
      minCount: 2,
    },
    {
      action: "centerX",
      zh: "水平居中",
      en: "Center X",
      icon: <AlignCenterVertical className="h-3.5 w-3.5" />,
      minCount: 2,
    },
    {
      action: "right",
      zh: "右对齐",
      en: "Align Right",
      icon: <AlignEndHorizontal className="h-3.5 w-3.5 -rotate-90" />,
      minCount: 2,
    },
    {
      action: "top",
      zh: "顶对齐",
      en: "Align Top",
      icon: <AlignStartHorizontal className="h-3.5 w-3.5" />,
      minCount: 2,
    },
    {
      action: "centerY",
      zh: "垂直居中",
      en: "Center Y",
      icon: <AlignCenterHorizontal className="h-3.5 w-3.5" />,
      minCount: 2,
    },
    {
      action: "distributeX",
      zh: "水平均分",
      en: "Distribute X",
      icon: <AlignHorizontalSpaceAround className="h-3.5 w-3.5" />,
      minCount: 3,
    },
    {
      action: "distributeY",
      zh: "垂直均分",
      en: "Distribute Y",
      icon: <AlignVerticalSpaceAround className="h-3.5 w-3.5" />,
      minCount: 3,
    },
  ];

  return (
    <div
      data-testid="selection-align-toolbar"
      className="pointer-events-auto inline-flex items-center gap-0.5 rounded-md border border-xblue/40 bg-card/95 px-1.5 py-1 shadow-sm backdrop-blur-xs"
    >
      <span className="px-1.5 font-mono-tech text-[11px] font-medium text-xblue tabular-nums">
        {selectedCount}
      </span>
      {actions.map((item) => {
        const label = tx(item.zh, item.en);
        return (
          <IconButton
            key={item.action}
            size="sm"
            disabled={selectedCount < item.minCount}
            data-testid={`align-btn-${item.action}`}
            title={label}
            aria-label={label}
            onClick={() => onAlign(item.action)}
          >
            {item.icon}
          </IconButton>
        );
      })}
    </div>
  );
}
