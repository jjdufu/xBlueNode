import { useI18n } from "@/i18n";
import type { GroundingMode } from "@/lib/api";
import { Badge } from "@/components/ui/primitives";

export interface GroundingBadgeProps {
  mode?: GroundingMode | null;
  className?: string;
}

export function GroundingBadge({ mode, className }: GroundingBadgeProps) {
  const { tx } = useI18n();

  if (!mode) {
    return null;
  }

  let label: string;
  let variant: "xblue" | "neutral" | "outline";

  switch (mode) {
    case "SEMANTIC_METRIC":
      label = tx("认证指标", "Metric");
      variant = "xblue";
      break;
    case "DOMAIN_OBJECT":
      label = tx("本体对象", "Ontology");
      variant = "xblue";
      break;
    case "GUARDED_SQL":
      label = tx("受控 SQL", "Guarded SQL");
      variant = "neutral";
      break;
    case "GROUNDED_DOC":
      label = tx("原文溯源", "Grounded Doc");
      variant = "neutral";
      break;
    case "ABSTAINED":
      label = tx("无依据拒答", "Abstained");
      variant = "outline";
      break;
    case "CLARIFICATION":
      label = tx("待澄清", "Clarify");
      variant = "outline";
      break;
    default:
      return null;
  }

  return (
    <Badge
      variant={variant}
      className={className}
      data-testid={`grounding-badge-${mode}`}
    >
      {label}
    </Badge>
  );
}
