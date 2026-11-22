import { useState } from "react";
import { Sparkles } from "lucide-react";
import { useI18n } from "@/i18n";
import {
  xblueviewApi,
  type DashboardDetail,
  type DashboardTheme,
} from "@/lib/api/xblueview";
import {
  Button,
  Input,
  Modal,
  Select,
  Textarea,
} from "@/components/ui/primitives";

export interface AiDashboardWizardProps {
  open: boolean;
  onClose: () => void;
  onGenerated: (dashboard: DashboardDetail) => void;
}

const EXAMPLE_PROMPTS_ZH = [
  "华东供应链库存与已支付订单监控看板",
  "工厂产能利用率、开放告警与设备三维孪生驾驶舱",
  "区域履约金额趋势与库存盘点回写看板",
];

const EXAMPLE_PROMPTS_EN = [
  "East China inventory and paid order monitoring dashboard",
  "Factory utilization, active alerts, and 3D equipment twin",
  "Regional fulfillment trend and inventory writeback board",
];

export function AiDashboardWizard({
  open,
  onClose,
  onGenerated,
}: AiDashboardWizardProps) {
  const { tx, isZh } = useI18n();
  const [prompt, setPrompt] = useState("");
  const [title, setTitle] = useState("");
  const [slug, setSlug] = useState("");
  const [theme, setTheme] = useState<DashboardTheme>("light");
  const [generating, setGenerating] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const examplePrompts = isZh ? EXAMPLE_PROMPTS_ZH : EXAMPLE_PROMPTS_EN;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!prompt.trim()) {
      setErrorMsg(
        tx("驾驶舱业务需求不能为空", "Dashboard prompt is required")
      );
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
    <Modal
      open={open}
      onClose={onClose}
      testId="ai-dashboard-wizard"
      maxWidthClass="max-w-lg"
      title={
        <>
          <Sparkles className="h-4 w-4 text-xblue shrink-0" />
          <span>{tx("AI 生成驾驶舱", "AI Dashboard Builder")}</span>
        </>
      }
      footer={
        <>
          <Button
            type="button"
            variant="outline"
            onClick={onClose}
            disabled={generating}
          >
            {tx("取消", "Cancel")}
          </Button>
          <Button
            type="submit"
            form="ai-dashboard-wizard-form"
            disabled={generating}
            data-testid="ai-wizard-submit"
          >
            <Sparkles className="h-3.5 w-3.5" />
            {generating
              ? tx("生成中…", "Generating…")
              : tx("生成草稿", "Generate")}
          </Button>
        </>
      }
    >
      <form
        id="ai-dashboard-wizard-form"
        onSubmit={(e) => void handleSubmit(e)}
        className="space-y-3 text-xs"
      >
        <div className="space-y-1">
          <label className="text-muted-foreground block">
            {tx("业务需求", "Prompt")}
          </label>
          <Textarea
            rows={3}
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder={tx(
              "华东供应链库存与已支付订单监控看板，含 KPI、状态分布与仓库 3D 孪生",
              "East China inventory and paid orders with KPI, status chart, and 3D warehouse twin"
            )}
            data-testid="ai-wizard-prompt"
          />
        </div>

        <div className="flex flex-wrap gap-1.5">
          {examplePrompts.map((ex) => (
            <button
              key={ex}
              type="button"
              onClick={() => setPrompt(ex)}
              className="rounded border border-border bg-muted/40 px-2 py-1 text-[11px] hover:border-xblue hover:text-xblue cursor-pointer text-left"
            >
              {ex}
            </button>
          ))}
        </div>

        <div className="grid grid-cols-1 sm:grid-cols-3 gap-2">
          <div className="space-y-1">
            <label className="text-muted-foreground block">
              {tx("标题", "Title")}
            </label>
            <Input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder={tx("供应链总览", "Supply Overview")}
              className="h-8 text-xs"
            />
          </div>
          <div className="space-y-1">
            <label className="text-muted-foreground block">Slug</label>
            <Input
              value={slug}
              onChange={(e) => setSlug(e.target.value)}
              placeholder="supply-ai"
              className="h-8 text-xs font-mono-tech"
            />
          </div>
          <div className="space-y-1">
            <label className="text-muted-foreground block">
              {tx("主题", "Theme")}
            </label>
            <Select
              value={theme}
              onChange={(e) => setTheme(e.target.value as DashboardTheme)}
              className="h-8 w-full text-xs"
            >
              <option value="light">{tx("浅色画布", "Light")}</option>
              <option value="dark">{tx("深色画布", "Dark")}</option>
              <option value="screen">{tx("大屏深色", "Big Screen")}</option>
            </Select>
          </div>
        </div>

        {errorMsg && (
          <div className="rounded border border-red-500/40 bg-red-500/10 px-2.5 py-1.5 text-red-700 dark:text-red-300">
            {errorMsg}
          </div>
        )}
      </form>
    </Modal>
  );
}
