import { useNavigate } from "react-router";
import {
  AlertTriangle,
  Boxes,
  FlaskConical,
  GitBranch,
  LayoutDashboard,
  ShieldCheck,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type { SubgraphPulseCard } from "@/lib/api/axops";
import type { RuntimeCertLevel } from "@/lib/api/enums";
import { Badge, Button, Card, EmptyState } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface SubgraphMatrixGridProps {
  subgraphs: SubgraphPulseCard[];
}

function certBadgeVariant(
  level: RuntimeCertLevel | null
): "success" | "xblue" | "warning" | "danger" | "outline" {
  switch (level) {
    case "L4_CERTIFIED":
      return "success";
    case "L3_VERIFIED":
    case "L2_RUNNABLE":
      return "xblue";
    case "L1_CONFIGURED":
      return "warning";
    case "L0_INVALID":
      return "danger";
    default:
      return "outline";
  }
}

function certShortCode(level: RuntimeCertLevel | null): string {
  if (!level) return "—";
  const prefix = level.split("_")[0];
  return prefix || level;
}

export function SubgraphMatrixGrid({ subgraphs }: SubgraphMatrixGridProps) {
  const navigate = useNavigate();
  const { tx, tEnum, fmtNum } = useI18n();

  return (
    <Card
      className="p-4 flex flex-col gap-3 h-full"
      data-testid="subgraph-matrix-grid"
    >
      <div className="flex items-center justify-between gap-2 border-b border-border pb-2.5">
        <div className="flex items-center gap-2 min-w-0">
          <Boxes className="h-4 w-4 text-xblue shrink-0" />
          <h2 className="text-sm font-semibold truncate">
            {tx("业务模型工作台矩阵", "Business Model Workbenches")}
          </h2>
        </div>
        <Badge variant="outline" className="font-mono-tech tabular-nums">
          {tx(`${fmtNum(subgraphs.length, 0)} 子图`, `${fmtNum(subgraphs.length, 0)} Subgraphs`)}
        </Badge>
      </div>

      {subgraphs.length === 0 ? (
        <EmptyState
          title={tx("暂无业务模型子图", "No business model subgraphs")}
          action={
            <Button
              type="button"
              variant="outline"
              size="sm"
              data-testid="subgraph-empty-studio-btn"
              onClick={() => void navigate("/studio")}
            >
              {tx("建模", "Model")}
            </Button>
          }
        />
      ) : (
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          {subgraphs.map((sg) => {
            const studioHref = `/studio?subgraph=${encodeURIComponent(sg.slug)}`;
            const simHref = sg.latest_sandbox_id
              ? `/sim?sandbox=${encodeURIComponent(sg.latest_sandbox_id)}`
              : `/sim?subgraph=${encodeURIComponent(sg.slug)}`;
            const viewHref = sg.primary_dashboard_slug
              ? `/view/${encodeURIComponent(sg.primary_dashboard_slug)}`
              : "/view";

            return (
              <div
                key={sg.slug}
                data-testid={`subgraph-card-${sg.slug}`}
                className={cn(
                  "rounded-md border p-3 flex flex-col justify-between gap-2.5 transition-colors",
                  sg.is_dev_sandbox
                    ? "border-dashed border-border bg-muted/25"
                    : "border-border bg-background/60 hover:border-xblue/45"
                )}
              >
                <div className="space-y-2">
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0">
                      <div className="flex items-center gap-1.5">
                        <span className="text-xs font-semibold text-foreground truncate">
                          {sg.display_name}
                        </span>
                        {sg.is_dev_sandbox && (
                          <Badge variant="outline">
                            {tx("演练", "Sandbox")}
                          </Badge>
                        )}
                      </div>
                      <div className="text-[11px] font-mono-tech text-muted-foreground truncate">
                        {sg.slug} · v{sg.schema_version}
                      </div>
                    </div>

                    <Badge
                      variant={certBadgeVariant(sg.cert_level)}
                      data-testid={`subgraph-cert-${sg.slug}`}
                      title={
                        sg.cert_level
                          ? tEnum(sg.cert_level)
                          : tx("未认证", "Uncertified")
                      }
                      className="font-mono-tech gap-1 shrink-0"
                    >
                      <ShieldCheck className="h-3 w-3" />
                      {sg.cert_level
                        ? `${certShortCode(sg.cert_level)}${
                            sg.readiness_score !== null
                              ? ` · ${sg.readiness_score}`
                              : ""
                          }`
                        : tx("待认证", "Unverified")}
                    </Badge>
                  </div>

                  <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-muted-foreground font-mono-tech tabular-nums">
                    <span data-testid={`subgraph-nodes-${sg.slug}`}>
                      {fmtNum(sg.node_count, 0)} {tx("节点", "nodes")}
                    </span>
                    <span data-testid={`subgraph-links-${sg.slug}`}>
                      {fmtNum(sg.link_count, 0)} {tx("边", "links")}
                    </span>
                    <span>
                      {fmtNum(sg.node_type_count, 0)} {tx("类型", "types")}
                    </span>
                    {sg.alert_count > 0 && (
                      <span
                        data-testid={`subgraph-alerts-${sg.slug}`}
                        className="inline-flex items-center gap-0.5 text-red-600 dark:text-red-400 font-medium"
                      >
                        <AlertTriangle className="h-3 w-3" />
                        {fmtNum(sg.alert_count, 0)} {tx("告警", "alerts")}
                      </span>
                    )}
                  </div>

                  {sg.business_domains.length > 0 && (
                    <div className="flex flex-wrap items-center gap-1">
                      {sg.business_domains.slice(0, 4).map((dom) => (
                        <Badge
                          key={dom}
                          variant="neutral"
                          className="text-[10px] px-1.5 py-0"
                        >
                          {dom}
                        </Badge>
                      ))}
                    </div>
                  )}
                </div>

                <div className="flex items-center gap-1.5 pt-2 border-t border-border/70">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="flex-1 h-7 px-2 text-[11px]"
                    data-testid={`subgraph-model-btn-${sg.slug}`}
                    onClick={() => void navigate(studioHref)}
                  >
                    <GitBranch className="h-3 w-3 text-xblue" />
                    {tx("建模", "Model")}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="flex-1 h-7 px-2 text-[11px]"
                    data-testid={`subgraph-sim-btn-${sg.slug}`}
                    onClick={() => void navigate(simHref)}
                  >
                    <FlaskConical className="h-3 w-3 text-xblue" />
                    {tx("推演", "Simulate")}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="flex-1 h-7 px-2 text-[11px]"
                    data-testid={`subgraph-view-btn-${sg.slug}`}
                    onClick={() => void navigate(viewHref)}
                  >
                    <LayoutDashboard className="h-3 w-3 text-xblue" />
                    {tx("看板", "Cockpit")}
                  </Button>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </Card>
  );
}
