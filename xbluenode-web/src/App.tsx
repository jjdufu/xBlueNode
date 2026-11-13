import { useState } from "react";
import { Moon, Sun } from "lucide-react";
import { MODULE_REGISTRY, type TabKey } from "@/modules/registry";
import { cn } from "@/lib/utils";

export function App() {
  const [tab, setTab] = useState<TabKey>("dashboard");
  const [dark, setDark] = useState(false);

  const toggleTheme = () => {
    const next = !dark;
    setDark(next);
    document.documentElement.classList.toggle("dark", next);
  };

  const activeModule = MODULE_REGISTRY.find((m) => m.key === tab) ?? MODULE_REGISTRY[0];
  const ActivePage = activeModule.component;

  return (
    <div className="min-h-screen flex bg-background text-foreground">
      <aside className="w-56 border-r border-border flex flex-col justify-between p-4 shrink-0">
        <div className="space-y-6">
          <div className="flex items-center justify-between px-2">
            <div>
              <div className="text-sm font-semibold tracking-tight flex items-center gap-1.5">
                <span className="inline-block h-2 w-2 rounded-full bg-xblue" />
                xBlueNode
              </div>
              <div className="text-[11px] font-mono-tech text-muted-foreground">
                AxPoint Core v{__APP_VERSION__}
              </div>
            </div>
          </div>

          <nav className="space-y-1">
            {MODULE_REGISTRY.map((item) => {
              const Icon = item.icon;
              const active = tab === item.key;
              return (
                <button
                  key={item.key}
                  type="button"
                  onClick={() => setTab(item.key)}
                  className={cn(
                    "w-full flex items-center gap-2.5 rounded-md px-2.5 py-2 text-xs font-medium transition-colors cursor-pointer",
                    active
                      ? "bg-muted text-foreground border border-border"
                      : "text-muted-foreground hover:text-foreground hover:bg-muted/50"
                  )}
                >
                  <Icon className={cn("h-3.5 w-3.5", active && "text-xblue")} />
                  {item.label}
                </button>
              );
            })}
          </nav>
        </div>

        <button
          type="button"
          onClick={toggleTheme}
          className="flex items-center gap-2 rounded-md border border-border px-2.5 py-2 text-xs text-muted-foreground hover:text-foreground cursor-pointer"
        >
          {dark ? <Sun className="h-3.5 w-3.5" /> : <Moon className="h-3.5 w-3.5" />}
          {dark ? "浅色极简模式" : "深色极简模式"}
        </button>
      </aside>

      <main className="flex-1 p-6 md:p-8 max-w-6xl mx-auto w-full">
        <ActivePage />
      </main>
    </div>
  );
}
