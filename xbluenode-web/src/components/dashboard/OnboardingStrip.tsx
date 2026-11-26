import { useState } from "react";
import { useNavigate } from "react-router";
import {
  ArrowRight,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Package,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type { OnboardingStatusResponse } from "@/lib/api/axops";
import {
  findFirstIncompleteStepKey,
  formatOnboardingStepMetric,
  isOnboardingComplete,
  resolveOnboardingNextTarget,
} from "@/components/axops/logic";
import { Badge, Button, Card } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface OnboardingStripProps {
  status: OnboardingStatusResponse | null;
  defaultExpanded?: boolean;
  hideTemplateAction?: boolean;
  onOpenTemplateHub: () => void;
}

export function OnboardingStrip({
  status,
  defaultExpanded = false,
  hideTemplateAction = false,
  onOpenTemplateHub,
}: OnboardingStripProps) {
  const navigate = useNavigate();
  const { isZh, tx } = useI18n();
  const [expandedWhenComplete, setExpandedWhenComplete] = useState(defaultExpanded);

  const steps = Array.isArray(status?.steps) ? status.steps : [];
  const totalSteps =
    typeof status?.total_steps === "number" && status.total_steps > 0
      ? status.total_steps
      : 5;
  const completedSteps =
    typeof status?.completed_steps === "number" && status.completed_steps >= 0
      ? status.completed_steps
      : 0;

  const allComplete = isOnboardingComplete(status);
  const firstIncompleteKey = findFirstIncompleteStepKey(status);
  const nextTargetPath = resolveOnboardingNextTarget(status);

  if (allComplete && !expandedWhenComplete) {
    return (
      <Card
        data-testid="onboarding-strip"
        className="px-3.5 py-2 flex flex-wrap items-center justify-between gap-2"
      >
        <div className="flex items-center gap-2">
          <Badge
            variant="success"
            data-testid="onboarding-completion-badge"
            className="gap-1 tabular-nums"
          >
            <CheckCircle2 className="h-3 w-3" />
            {tx(
              `${completedSteps} / ${totalSteps} 已就绪`,
              `${completedSteps} / ${totalSteps} Ready`
            )}
          </Badge>
          {status?.has_trial_subgraph && (
            <Badge variant="xblue">{tx("演练沙盒", "Trial Sandbox")}</Badge>
          )}
        </div>

        <div className="flex items-center gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-testid="onboarding-try-template-btn"
            onClick={onOpenTemplateHub}
          >
            <Package className="h-3.5 w-3.5 text-xblue" />
            {tx("试用模板", "Try Template")}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            data-testid="onboarding-expand-toggle"
            onClick={() => setExpandedWhenComplete(true)}
          >
            <ChevronDown className="h-3.5 w-3.5" />
            {tx("展开", "Expand")}
          </Button>
        </div>
      </Card>
    );
  }

  return (
    <Card
      data-testid="onboarding-strip"
      className="px-3.5 py-2.5 flex flex-wrap xl:flex-nowrap items-center justify-between gap-3"
    >
      {/* Left: Completion Ratio Badge */}
      <div className="flex items-center gap-2 shrink-0">
        <Badge
          variant={allComplete ? "success" : "xblue"}
          data-testid="onboarding-completion-badge"
          className="gap-1 tabular-nums"
        >
          <CheckCircle2 className="h-3 w-3" />
          {tx(
            `${completedSteps} / ${totalSteps} 已就绪`,
            `${completedSteps} / ${totalSteps} Ready`
          )}
        </Badge>
        {status?.has_trial_subgraph && (
          <Badge variant="outline">{tx("演练沙盒", "Trial Sandbox")}</Badge>
        )}
      </div>

      {/* Center: 5 Equal Step Pills */}
      {steps.length > 0 && (
        <div className="grid grid-cols-1 sm:grid-cols-5 gap-1.5 flex-1 min-w-0 w-full xl:w-auto">
          {steps.map((step) => {
            const isFirstIncomplete =
              !step.completed && step.step_key === firstIncompleteKey;
            const metricText = formatOnboardingStepMetric(step, isZh);
            return (
              <button
                key={step.step_key}
                type="button"
                data-testid={`onboarding-step-${step.step_key}`}
                onClick={() => void navigate(step.target_path)}
                className={cn(
                  "flex items-center gap-2 rounded-md border px-2.5 py-1.5 text-left transition-colors cursor-pointer min-w-0",
                  step.completed
                    ? "border-xblue/35 bg-xblue-soft/35 hover:border-xblue"
                    : isFirstIncomplete
                      ? "border-xblue bg-background ring-1 ring-xblue/25"
                      : "border-border bg-background/60 hover:border-foreground/35"
                )}
              >
                {step.completed ? (
                  <span className="inline-flex h-4 w-4 items-center justify-center rounded-full bg-xblue text-white shrink-0">
                    <Check className="h-2.5 w-2.5" />
                  </span>
                ) : (
                  <span
                    className={cn(
                      "inline-flex h-4 w-4 items-center justify-center rounded-full text-[10px] font-mono-tech font-semibold shrink-0",
                      isFirstIncomplete
                        ? "bg-xblue-soft text-xblue border border-xblue/40"
                        : "bg-muted text-muted-foreground"
                    )}
                  >
                    {step.step_no}
                  </span>
                )}
                <div className="min-w-0 flex-1">
                  <div className="text-[11px] font-medium text-foreground truncate leading-tight">
                    {isZh ? step.label_zh : step.label_en}
                  </div>
                  <div className="text-[10px] font-mono-tech text-muted-foreground truncate leading-tight mt-0.5">
                    {metricText}
                  </div>
                </div>
              </button>
            );
          })}
        </div>
      )}

      {/* Right: Primary Continue + Secondary Try Template */}
      <div className="flex items-center gap-2 shrink-0 ml-auto xl:ml-0">
        {!allComplete && (
          <Button
            type="button"
            variant="primary"
            size="sm"
            data-testid="onboarding-continue-btn"
            onClick={() => void navigate(nextTargetPath)}
          >
            {tx("继续构建", "Continue")}
            <ArrowRight className="h-3.5 w-3.5" />
          </Button>
        )}
        {!hideTemplateAction && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-testid="onboarding-try-template-btn"
            onClick={onOpenTemplateHub}
          >
            <Package className="h-3.5 w-3.5 text-xblue" />
            {tx("试用模板", "Try Template")}
          </Button>
        )}
        {allComplete && expandedWhenComplete && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            data-testid="onboarding-expand-toggle"
            onClick={() => setExpandedWhenComplete(false)}
          >
            <ChevronUp className="h-3.5 w-3.5" />
            {tx("收起", "Collapse")}
          </Button>
        )}
      </div>
    </Card>
  );
}
