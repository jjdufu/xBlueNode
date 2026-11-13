import { type FormEvent, useState } from "react";
import { KeyRound, Moon, ShieldCheck, Sun } from "lucide-react";
import { useNavigate } from "react-router";
import { ApiError, fetchMe, setAccessToken } from "@/lib/api";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

export function LoginPage() {
  const navigate = useNavigate();
  const [tokenInput, setTokenInput] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [dark, setDark] = useState(
    () =>
      typeof document !== "undefined" &&
      document.documentElement.classList.contains("dark")
  );

  const toggleTheme = () => {
    const next = !dark;
    setDark(next);
    document.documentElement.classList.toggle("dark", next);
  };

  const handleSubmit = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const candidate = tokenInput.trim();
    if (!candidate) {
      setError("请粘贴有效的访问令牌");
      return;
    }

    setSubmitting(true);
    setError(null);
    try {
      await fetchMe(candidate);
      setAccessToken(candidate);
      void navigate("/", { replace: true });
    } catch (err) {
      if (err instanceof ApiError) {
        setError(`[${err.errorCode}] ${err.message}`);
      } else {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="min-h-screen flex flex-col justify-between bg-background text-foreground p-6">
      <header className="flex items-center justify-between max-w-5xl w-full mx-auto">
        <div>
          <div className="text-sm font-semibold tracking-tight flex items-center gap-1.5">
            <span className="inline-block h-2 w-2 rounded-full bg-xblue" />
            xBlueNode
          </div>
          <div className="text-[11px] font-mono-tech text-muted-foreground">
            AxPoint Core v{__APP_VERSION__}
          </div>
        </div>
        <Button variant="outline" type="button" onClick={toggleTheme}>
          {dark ? <Sun className="h-3.5 w-3.5" /> : <Moon className="h-3.5 w-3.5" />}
          {dark ? "浅色极简模式" : "深色极简模式"}
        </Button>
      </header>

      <main className="w-full max-w-md mx-auto my-auto py-8">
        <Card className="p-6 space-y-5">
          <div className="space-y-1.5 border-b border-border pb-4">
            <div className="flex items-center justify-between">
              <h1 className="text-lg font-semibold tracking-tight flex items-center gap-2">
                <ShieldCheck className="h-4 w-4 text-xblue" />
                AxGuard · 访问令牌认证
              </h1>
              <Badge variant="xblue">BEARER</Badge>
            </div>
            <p className="text-xs text-muted-foreground leading-relaxed">
              粘贴访问令牌以校验身份并进入工作台，令牌仅驻留于浏览器当前会话内存。
            </p>
          </div>

          <form onSubmit={(e) => void handleSubmit(e)} className="space-y-4">
            <div className="space-y-2">
              <label
                htmlFor="access-token-input"
                className="flex items-center gap-1.5 text-xs font-medium"
              >
                <KeyRound className="h-3.5 w-3.5 text-xblue" />
                访问令牌（Access Token）
              </label>
              <Input
                id="access-token-input"
                type="password"
                autoComplete="off"
                placeholder="粘贴 AXPOINT_DEV_TOKEN 或 axp_ 个人访问令牌..."
                value={tokenInput}
                onChange={(e) => setTokenInput(e.target.value)}
                className="font-mono-tech text-xs"
              />
            </div>

            {error && (
              <div
                role="alert"
                className="rounded-md border border-foreground/30 bg-muted/60 px-3 py-2.5 text-xs"
              >
                <span className="font-mono-tech font-semibold">认证失败：</span>
                <span>{error}</span>
              </div>
            )}

            <Button
              type="submit"
              className="w-full"
              disabled={submitting || !tokenInput.trim()}
            >
              {submitting ? "正在校验令牌 (/api/auth/me)..." : "验证并进入工作台"}
            </Button>
          </form>
        </Card>
      </main>

      <footer className="text-center text-[11px] font-mono-tech text-muted-foreground">
        xBlueNode / AxPoint · Tenant Isolation Enforced via AuthPrincipal &amp; PostgreSQL RLS
      </footer>
    </div>
  );
}
