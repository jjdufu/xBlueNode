import { useState } from "react";
import { Sparkles, X } from "lucide-react";
import {
  xblueviewApi,
  type DashboardDetail,
  type DashboardTheme,
} from "@/lib/api/xblueview";
import { Button, Input, Textarea } from "@/components/ui/primitives";

export interface AiDashboardWizardProps {
  open: boolean;
  onClose: () => void;
  onGenerated: (dashboard: DashboardDetail) => void;
}

const EXAMPLE_PROMPTS = [
  "生成华东供应链库存与已支付订单监控看板",
  "构建工厂产能利用率、开放告警与设备三维孪生驾驶舱",
  "生成区域履约金额趋势与库存盘点回写运营看板",
];

export function AiDashboardWizard({
  open,
  onClose,
  onGenerated,
}: AiDashboardWizardProps) {
  const [prompt, setPrompt] = useState("");
  const [title, setTitle] = useState("");
  const [slug, setSlug] = useState("");
  const [theme, setTheme] = useState<DashboardTheme>("light");
  const [generating, setGenerating] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  if (!open) return null;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!prompt.trim()) {
      setErrorMsg("请输入驾驶舱生成描述");
      return;
    }
    setGenerating(true);
    setErrorMsg(null);
    try {
      const detail = await xblueviewApi.aiGenerateDashboard({
        prompt: prompt.trim(),
        title: title.trim() || undefined,
        slug: slug.trim() || undefined,
        theme,
      });
      onGenerated(detail);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setGenerating(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-lg rounded-md border border-border bg-background shadow-xl overflow-hidden"
        data-testid="ai-dashboard-wizard"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="px-4 py-3 border-b border-border flex items-center justify-between">
          <h3 className="text-sm font-semibold flex items-center gap-2">
            <Sparkles className="h-4 w-4 text-xblue" />
            AI 一句话生成孪生驾驶舱
          </h3>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <form onSubmit={(e) => void handleSubmit(e)} className="p-4 space-y-3 text-xs">
          <div className="space-y-1">
            <label className="text-muted-foreground block">
              业务需求描述（自动感知主图 Schema、指标、湖表与 3D 资产）
            </label>
            <Textarea
              rows={3}
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              placeholder="例如：生成华东供应链库存与已支付订单监控看板，包含 KPI、状态分布与仓库 3D 孪生..."
              data-testid="ai-wizard-prompt"
            />
          </div>

          <div className="flex flex-wrap gap-1.5">
            {EXAMPLE_PROMPTS.map((ex) => (
              <button
                key={ex}
                type="button"
                onClick={() => setPrompt(ex)}
                className="rounded border border-border bg-muted/40 px-2 py-1 text-[11px] hover:border-xblue hover:text-xblue cursor-pointer"
              >
                {ex}
              </button>
            ))}
          </div>

          <div className="grid grid-cols-3 gap-2">
            <div className="space-y-1">
              <label className="text-muted-foreground block">标题（可选）</label>
              <Input
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder="自动生成"
                className="h-8 text-xs"
              />
            </div>
            <div className="space-y-1">
              <label className="text-muted-foreground block">Slug（可选）</label>
              <Input
                value={slug}
                onChange={(e) => setSlug(e.target.value)}
                placeholder="supply-ai"
                className="h-8 text-xs font-mono-tech"
              />
            </div>
            <div className="space-y-1">
              <label className="text-muted-foreground block">主题</label>
              <select
                value={theme}
                onChange={(e) => setTheme(e.target.value as DashboardTheme)}
                className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
              >
                <option value="light">浅色极简 (light)</option>
                <option value="dark">深色极简 (dark)</option>
                <option value="screen">大屏模式 (screen)</option>
              </select>
            </div>
          </div>

          {errorMsg && (
            <div className="rounded border border-foreground/30 bg-muted px-2.5 py-1.5">
              {errorMsg}
            </div>
          )}

          <div className="flex justify-end gap-2 pt-2 border-t border-border">
            <Button type="button" variant="outline" onClick={onClose}>
              取消
            </Button>
            <Button
              type="submit"
              disabled={generating}
              data-testid="ai-wizard-submit"
            >
              <Sparkles className="h-3.5 w-3.5" />
              {generating ? "正在生成草稿..." : "生成驾驶舱草稿"}
            </Button>
          </div>
        </form>
      </div>
    </div>
  );
}
