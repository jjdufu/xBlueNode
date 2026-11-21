import { type FormEvent, useEffect, useState } from "react";
import {
  Building2,
  ChevronDown,
  ChevronRight,
  Globe,
  KeyRound,
  Lock,
  Moon,
  ShieldCheck,
  Sun,
  User,
} from "lucide-react";
import { useLocation, useNavigate } from "react-router";
import {
  ApiError,
  DEFAULT_AUTH_CONFIG,
  buildOidcStartUrl,
  fetchAuthConfig,
  fetchMe,
  loginWithPassword,
  setPatAccessToken,
  type AuthConfigResponse,
} from "@/lib/api";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

function formatLoginError(err: unknown): { code: string; detail: string } {
  if (err instanceof ApiError) {
    switch (err.errorCode) {
      case "INVALID_CREDENTIALS":
        return {
          code: "INVALID_CREDENTIALS",
          detail:
            "租户、用户名或口令不正确，请核对后重试（连续 5 次失败将自动锁定账号 15 分钟）。",
        };
      case "ACCOUNT_LOCKED":
        return {
          code: "ACCOUNT_LOCKED",
          detail:
            "连续登录失败已达阈值，账号已被安全锁定 15 分钟；请稍后再试或联系安全管理员解锁。",
        };
      case "ACCOUNT_DISABLED":
        return {
          code: "ACCOUNT_DISABLED",
          detail: "当前账号已被停用，请联系租户安全管理员启用账号。",
        };
      case "RATE_LIMITED":
        return {
          code: "RATE_LIMITED",
          detail: "登录请求过于频繁（已触发 IP 速率保护），请稍候再试。",
        };
      default:
        return {
          code: err.errorCode,
          detail: err.message,
        };
    }
  }
  return {
    code: "AUTH_ERROR",
    detail: err instanceof Error ? err.message : String(err),
  };
}

export function LoginPage() {
  const navigate = useNavigate();
  const location = useLocation();
  const searchParams = new URLSearchParams(location.search);
  const rawReturnTo = searchParams.get("return_to") ?? "/";
  const returnTo =
    rawReturnTo.startsWith("/") && !rawReturnTo.startsWith("//")
      ? rawReturnTo
      : "/";

  const [authConfig, setAuthConfig] =
    useState<AuthConfigResponse>(DEFAULT_AUTH_CONFIG);
  const [tenant, setTenant] = useState(DEFAULT_AUTH_CONFIG.default_tenant);
  const [tenantEdited, setTenantEdited] = useState(false);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [loginError, setLoginError] = useState<{
    code: string;
    detail: string;
  } | null>(null);

  const [patOpen, setPatOpen] = useState(false);
  const [tokenInput, setTokenInput] = useState("");
  const [patSubmitting, setPatSubmitting] = useState(false);
  const [patError, setPatError] = useState<string | null>(null);

  const [dark, setDark] = useState(
    () =>
      typeof document !== "undefined" &&
      document.documentElement.classList.contains("dark")
  );

  useEffect(() => {
    let cancelled = false;
    void fetchAuthConfig().then((cfg) => {
      if (cancelled) {
        return;
      }
      setAuthConfig(cfg);
      if (!tenantEdited && cfg.default_tenant) {
        setTenant(cfg.default_tenant);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [tenantEdited]);

  const toggleTheme = () => {
    const next = !dark;
    setDark(next);
    document.documentElement.classList.toggle("dark", next);
  };

  const handlePasswordLogin = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const cleanTenant = tenant.trim() || "default";
    const cleanUsername = username.trim();
    if (!cleanUsername || !password) {
      setLoginError({
        code: "INVALID_ARGUMENT",
        detail: "请输入用户名与口令",
      });
      return;
    }

    setSubmitting(true);
    setLoginError(null);
    try {
      await loginWithPassword({
        tenant: cleanTenant,
        username: cleanUsername,
        password,
      });
      await fetchMe();
      void navigate(returnTo, { replace: true });
    } catch (err) {
      setLoginError(formatLoginError(err));
    } finally {
      setSubmitting(false);
    }
  };

  const handleOidcLogin = () => {
    const targetUrl = buildOidcStartUrl(tenant, returnTo);
    window.location.assign(targetUrl);
  };

  const handlePatSubmit = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const candidate = tokenInput.trim();
    if (!candidate) {
      setPatError("请粘贴有效的访问令牌");
      return;
    }

    setPatSubmitting(true);
    setPatError(null);
    try {
      await fetchMe(candidate);
      setPatAccessToken(candidate);
      void navigate(returnTo, { replace: true });
    } catch (err) {
      if (err instanceof ApiError) {
        setPatError(`[${err.errorCode}] ${err.message}`);
      } else {
        setPatError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      setPatSubmitting(false);
    }
  };

  const isProduction = authConfig.env === "production";

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
                AxGuard · 统一身份认证
              </h1>
              <Badge variant="xblue">EdDSA / RLS</Badge>
            </div>
            <p className="text-xs text-muted-foreground leading-relaxed">
              使用租户账号口令或企业单点登录进入工作台；访问令牌仅驻留于浏览器内存并自动静默轮换。
            </p>
          </div>

          <form
            onSubmit={(e) => void handlePasswordLogin(e)}
            className="space-y-3.5"
            data-testid="password-login-form"
          >
            <div className="space-y-1.5">
              <label
                htmlFor="login-tenant-input"
                className="flex items-center gap-1.5 text-xs font-medium"
              >
                <Building2 className="h-3.5 w-3.5 text-muted-foreground" />
                租户标识（Tenant）
              </label>
              <Input
                id="login-tenant-input"
                data-testid="login-tenant-input"
                type="text"
                autoComplete="organization"
                placeholder="default"
                value={tenant}
                onChange={(e) => {
                  setTenantEdited(true);
                  setTenant(e.target.value);
                }}
                className="font-mono-tech text-xs h-8"
              />
            </div>

            <div className="space-y-1.5">
              <label
                htmlFor="login-username-input"
                className="flex items-center gap-1.5 text-xs font-medium"
              >
                <User className="h-3.5 w-3.5 text-muted-foreground" />
                用户名
              </label>
              <Input
                id="login-username-input"
                data-testid="login-username-input"
                type="text"
                autoComplete="username"
                placeholder="请输入账号名（如 ops_admin）"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                className="text-xs h-9"
              />
            </div>

            <div className="space-y-1.5">
              <label
                htmlFor="login-password-input"
                className="flex items-center gap-1.5 text-xs font-medium"
              >
                <Lock className="h-3.5 w-3.5 text-muted-foreground" />
                口令
              </label>
              <Input
                id="login-password-input"
                data-testid="login-password-input"
                type="password"
                autoComplete="current-password"
                placeholder="请输入登录口令"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                className="text-xs h-9"
              />
            </div>

            {loginError && (
              <div
                role="alert"
                data-testid="login-error-banner"
                className="rounded-md border border-foreground/30 bg-muted/70 px-3 py-2.5 text-xs space-y-1"
              >
                <div className="font-mono-tech font-semibold text-foreground">
                  认证失败 [{loginError.code}]
                </div>
                <div className="text-muted-foreground leading-relaxed">
                  {loginError.detail}
                </div>
              </div>
            )}

            <Button
              type="submit"
              data-testid="login-submit-btn"
              className="w-full"
              disabled={submitting || !username.trim() || !password}
            >
              {submitting ? "正在核验身份..." : "登录进入工作台"}
            </Button>
          </form>

          {authConfig.oidc_enabled && (
            <div className="space-y-3 pt-1">
              <div className="relative flex items-center justify-center">
                <div className="border-t border-border w-full" />
                <span className="bg-card px-2 text-[11px] text-muted-foreground shrink-0">
                  或联邦身份认证
                </span>
                <div className="border-t border-border w-full" />
              </div>

              <Button
                type="button"
                variant="outline"
                data-testid="oidc-login-btn"
                onClick={handleOidcLogin}
                className="w-full border-xblue/40 hover:border-xblue text-foreground"
              >
                <Globe className="h-3.5 w-3.5 text-xblue" />
                {authConfig.oidc_display_name || "企业单点登录 (OIDC)"}
              </Button>
            </div>
          )}

          <div className="border-t border-border pt-3">
            <button
              type="button"
              data-testid="pat-accordion-toggle"
              aria-expanded={patOpen}
              onClick={() => setPatOpen((prev) => !prev)}
              className="w-full flex items-center justify-between rounded-md px-2 py-1.5 text-xs font-medium text-muted-foreground hover:text-foreground hover:bg-muted/50 transition-colors cursor-pointer"
            >
              <span className="flex items-center gap-1.5">
                <KeyRound className="h-3.5 w-3.5 text-xblue" />
                访问令牌 (PAT) 登录
              </span>
              {patOpen ? (
                <ChevronDown className="h-3.5 w-3.5" />
              ) : (
                <ChevronRight className="h-3.5 w-3.5" />
              )}
            </button>

            {patOpen && (
              <form
                onSubmit={(e) => void handlePatSubmit(e)}
                className="mt-3 space-y-3 rounded-md border border-border bg-muted/30 p-3"
                data-testid="pat-login-form"
              >
                <div className="space-y-1.5">
                  <label
                    htmlFor="access-token-input"
                    className="block text-[11px] font-medium text-muted-foreground"
                  >
                    {isProduction
                      ? "个人访问令牌（axp_...）"
                      : "个人访问令牌（axp_...）或调试静态令牌"}
                  </label>
                  <Input
                    id="access-token-input"
                    type="password"
                    autoComplete="off"
                    placeholder={
                      isProduction
                        ? "粘贴 axp_ 个人访问令牌..."
                        : "粘贴 AXPOINT_DEV_TOKEN 或 axp_ 个人访问令牌..."
                    }
                    value={tokenInput}
                    onChange={(e) => setTokenInput(e.target.value)}
                    className="font-mono-tech text-xs h-8"
                  />
                </div>

                {patError && (
                  <div
                    role="alert"
                    className="rounded-md border border-foreground/30 bg-muted/60 px-2.5 py-2 text-xs"
                  >
                    <span className="font-mono-tech font-semibold">令牌校验失败：</span>
                    <span>{patError}</span>
                  </div>
                )}

                <Button
                  type="submit"
                  variant="outline"
                  data-testid="pat-submit-btn"
                  className="w-full"
                  disabled={patSubmitting || !tokenInput.trim()}
                >
                  {patSubmitting
                    ? "正在校验令牌 (/api/auth/me)..."
                    : "验证并进入工作台"}
                </Button>
              </form>
            )}
          </div>
        </Card>
      </main>

      <footer className="text-center text-[11px] font-mono-tech text-muted-foreground">
        xBlueNode / AxPoint · Tenant Isolation Enforced via AuthPrincipal, Cedar ABAC &amp; PostgreSQL RLS
      </footer>
    </div>
  );
}
