import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { ArrowRight, Search, X } from "lucide-react";
import { useI18n } from "@/i18n";
import { useTheme } from "@/lib/theme";
import { MODULE_META } from "@/modules/meta";

export interface CommandPaletteProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
}

interface QuickJumpItem {
  readonly id: string;
  readonly zh: string;
  readonly en: string;
  readonly keywords: string;
  readonly path?: string;
  readonly action?: "theme" | "locale";
}

const DEFAULT_JUMP_ITEMS: readonly QuickJumpItem[] = [
  { id: "nav-dashboard", zh: MODULE_META.dashboard.label, en: MODULE_META.dashboard.enLabel, keywords: "overview home 概览 向导", path: "/" },
  { id: "nav-hub", zh: MODULE_META.axhub.label, en: MODULE_META.axhub.enLabel, keywords: "hub dataset connector knowledge 数据与知识 连接源 数据集 血缘", path: "/hub" },
  { id: "nav-hub-knowledge", zh: "知识文档", en: "Knowledge Docs", keywords: "ingest docs upload 知识文档 上传", path: "/hub?tab=knowledge" },
  { id: "nav-analytics", zh: MODULE_META.axanalytics.label, en: MODULE_META.axanalytics.enLabel, keywords: "explore sql chart metric 探索与制图 问数 指标", path: "/analytics" },
  { id: "nav-analytics-pivot", zh: "透视分析", en: "Pivot Analysis", keywords: "pivot vtable 透视", path: "/analytics?tab=pivot" },
  { id: "nav-studio", zh: MODULE_META.xbluestudio.label, en: MODULE_META.xbluestudio.enLabel, keywords: "studio ontology schema 本体工作室 建模", path: "/studio" },
  { id: "nav-studio-explorer", zh: "图谱浏览", en: "Graph Explorer", keywords: "graph explorer 图谱浏览 拓扑", path: "/studio?tab=explorer" },
  { id: "nav-flow", zh: MODULE_META.axflow.label, en: MODULE_META.axflow.enLabel, keywords: "flow alerts approval 编排与告警 审批", path: "/flow" },
  { id: "nav-agents", zh: MODULE_META.axagent.label, en: MODULE_META.axagent.enLabel, keywords: "agents mcp api im 智能体中心 接口 模型", path: "/agents" },
  { id: "nav-sim", zh: MODULE_META.axsim.label, en: MODULE_META.axsim.enLabel, keywords: "sim sandbox scenario 推演沙盘 因果", path: "/sim" },
  { id: "nav-view", zh: MODULE_META.xblueview.label, en: MODULE_META.xblueview.enLabel, keywords: "view cockpit export 孪生看板 出品", path: "/view" },
  { id: "nav-governance", zh: MODULE_META.governance.label, en: MODULE_META.governance.enLabel, keywords: "governance security masking ops backup 系统治理 脱敏 审计 备份", path: "/governance" },
  { id: "act-theme", zh: "切换亮暗主题", en: "Toggle Theme", keywords: "theme dark light 主题 暗色 亮色", action: "theme" },
  { id: "act-locale", zh: "切换中英语言", en: "Switch Language", keywords: "locale language 中文 english 语言", action: "locale" },
];

export function CommandPalette({ open, onOpenChange }: CommandPaletteProps) {
  const navigate = useNavigate();
  const { isZh, toggleLocale, tx } = useI18n();
  const { toggleTheme } = useTheme();
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        onOpenChange(!open);
      } else if (open && e.key === "Escape") {
        e.preventDefault();
        onOpenChange(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, onOpenChange]);

  useEffect(() => {
    if (open) {
      setQuery("");
      setSelectedIndex(0);
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return DEFAULT_JUMP_ITEMS;
    return DEFAULT_JUMP_ITEMS.filter(
      (item) =>
        item.zh.toLowerCase().includes(q) ||
        item.en.toLowerCase().includes(q) ||
        item.keywords.toLowerCase().includes(q)
    );
  }, [query]);

  if (!open) {
    return null;
  }

  const executeItem = (item: QuickJumpItem | undefined) => {
    if (!item) return;
    onOpenChange(false);
    if (item.action === "theme") {
      toggleTheme();
      return;
    }
    if (item.action === "locale") {
      toggleLocale();
      return;
    }
    if (item.path) {
      void navigate(item.path);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/45 pt-[14vh] px-4 backdrop-blur-xs"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) {
          onOpenChange(false);
        }
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={tx("全局搜索与命令", "Command Palette")}
        data-testid="global-cmdk-dialog"
        className="w-full max-w-xl overflow-hidden rounded-xl border border-border bg-card text-card-foreground shadow-2xl"
      >
        <div className="flex items-center gap-2.5 border-b border-border px-3.5 py-2.5">
          <Search className="h-4 w-4 text-muted-foreground shrink-0" />
          <input
            ref={inputRef}
            type="text"
            data-testid="global-cmdk-input"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setSelectedIndex(0);
            }}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setSelectedIndex((prev) =>
                  filtered.length === 0 ? 0 : (prev + 1) % filtered.length
                );
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setSelectedIndex((prev) =>
                  filtered.length === 0
                    ? 0
                    : (prev - 1 + filtered.length) % filtered.length
                );
              } else if (e.key === "Enter") {
                e.preventDefault();
                executeItem(filtered[selectedIndex]);
              }
            }}
            placeholder={tx("搜索视图、模型与图表…", "Search views, models, charts…")}
            className="flex-1 bg-transparent text-[13px] text-foreground placeholder:text-muted-foreground focus:outline-none"
          />
          <button
            type="button"
            onClick={() => onOpenChange(false)}
            aria-label={tx("关闭", "Close")}
            className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>

        <div className="max-h-80 overflow-y-auto p-1.5">
          {filtered.length === 0 ? (
            <div className="py-8 text-center text-xs text-muted-foreground">
              {tx("无匹配项", "No matching commands")}
            </div>
          ) : (
            filtered.map((item, idx) => {
              const active = idx === selectedIndex;
              return (
                <button
                  key={item.id}
                  type="button"
                  data-testid={`cmdk-item-${item.id}`}
                  onMouseEnter={() => setSelectedIndex(idx)}
                  onClick={() => executeItem(item)}
                  className={`w-full flex items-center justify-between rounded-lg px-3 py-2 text-[13px] transition-colors cursor-pointer ${
                    active
                      ? "bg-xblue-soft text-xblue font-medium"
                      : "text-foreground hover:bg-muted/70"
                  }`}
                >
                  <span>{isZh ? item.zh : item.en}</span>
                  <ArrowRight className="h-3.5 w-3.5 opacity-60 shrink-0" />
                </button>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}
