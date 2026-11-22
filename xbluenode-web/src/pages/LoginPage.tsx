import { type FormEvent, useEffect, useState } from "react";
import {
  Building2,
  ChevronDown,
  ChevronRight,
  Globe,
  KeyRound,
  Languages,
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
import {
  Button,
  Card,
  IconButton,
  Input,
  PasswordInput,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { useTheme } from "@/lib/theme";

function formatLoginError(
  err: unknown,
  tx: (zh: string, en: string) => string
): { code: string; detail: string } {
  if (err instanceof ApiError) {
    switch (err.errorCode) {
      case "INVALID_CREDENTIALS":
        return {
          code: "INVALID_CREDENTIALS",
          detail: tx(
            "租户、用户名或口令不正确，连续 5 次失败将锁定 15 分钟。",
            "Invalid tenant, username, or password. 5 failed attempts lock the account for 15 minutes."
          ),
        };
      case "ACCOUNT_LOCKED":
        return {
          code: "ACCOUNT_LOCKED",
          detail: tx(
            "账号已锁定 15 分钟，请稍后重试或联系管理员解锁。",
            "Account locked for 15 minutes. Try again later or contact an administrator."
          ),
        };
      case "ACCOUNT_DISABLED":
        return {
          code: "ACCOUNT_DISABLED",
          detail: tx(
            "账号已停用，请联系管理员。",
            "Account is disabled. Contact your administrator."
          ),
        };
      case "RATE_LIMITED":
        return {
          code: "RATE_LIMITED",
          detail: tx(
            "请求过于频繁，请稍后重试。",
            "Too many sign-in attempts. Please wait and try again."
          ),
        };
      default:
        return {
          code: err.errorCode,
          detail: err.message,
        };
    }
  }
  return {
    code: "NETWORK_ERROR",
    detail: err instanceof Error ? err.message : String(err),
  };
}

function resolveReturnPath(search: string): string {
  const params = new URLSearchParams(search);
  const raw = params.get("return_to")?.trim() ?? "";
  if (raw.startsWith("/") && !raw.startsWith("//") && !raw.startsWith("/login")) {
    return raw;
  }
  return "/";
}

export function LoginPage() {
  const location = useLocation();
  const navigate = useNavigate();
  const returnTo = resolveReturnPath(location.search);
  const { isDark, toggleTheme } = useTheme();
  const { isZh, toggleLocale, tx } = useI18n();

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

  const handlePasswordLogin = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const cleanTenant = tenant.trim() || "default";
    const cleanUsername = username.trim();
    if (!cleanUsername || !password) {
      setLoginError({
        code: "INVALID_ARGUMENT",
        detail: tx("请填写用户名与口令", "Username and password are required"),
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
      setLoginError(formatLoginError(err, tx));
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
      setPatError(tx("请填写访问令牌", "Access token is required"));
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

  return (
    <div className="min-h-screen flex flex-col bg-background text-foreground">
      {/* Unified Sticky Top Header: Top-Left Brand + Locale, Top-Right Borderless Theme Icon */}
      <header className="sticky top-0 z-30 h-12 border-b border-border bg-card/95 backdrop-blur-xs px-4 flex items-center justify-between">
        <div className="flex items-center gap-2.5">
          <div className="flex items-center gap-1.5 select-none">
            <span className="inline-block h-2 w-2 rounded-full bg-xblue" />
            <span className="text-sm font-semibold tracking-tight">
              xBlueNode
            </span>
            <span className="text-[11px] font-mono-tech text-muted-foreground">
              v{__APP_VERSION__}
            </span>
          </div>

          <button
            type="button"
            data-testid="locale-toggle-btn"
            onClick={toggleLocale}
            aria-label={tx("切换语言", "Switch language")}
            title={tx("切换为 English", "Switch to 简体中文")}
            className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] font-medium text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer"
          >
            <Languages className="h-3.5 w-3.5 text-xblue" />
            <span className="font-mono-tech">{isZh ? "中文" : "EN"}</span>
          </button>
        </div>

        <div className="flex items-center gap-1.5">
          <IconButton
            data-testid="theme-toggle-btn"
            onClick={toggleTheme}
            aria-label={
              isDark
                ? tx("切换亮色模式", "Switch to light mode")
                : tx("切换暗色模式", "Switch to dark mode")
            }
            title={
              isDark
                ? tx("亮色模式", "Light mode")
                : tx("暗色模式", "Dark mode")
            }
          >
            {isDark ? (
              <Sun className="h-4 w-4" />
            ) : (
              <Moon className="h-4 w-4" />
            )}
          </IconButton>
        </div>
      </header>

      <main className="w-full max-w-md mx-auto my-auto px-4 py-10">
        <Card className="p-6 space-y-5 shadow-xs">
          <div className="flex items-center justify-between border-b border-border pb-3.5">
            <h1 className="text-base font-semibold tracking-tight flex items-center gap-2">
              <ShieldCheck className="h-4 w-4 text-xblue shrink-0" />
              {tx("AxGuard · 统一身份认证", "AxGuard · Sign In")}
            </h1>
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
                {tx("租户", "Tenant")}
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
                className="font-mono-tech h-9"
              />
            </div>

            <div className="space-y-1.5">
              <label
                htmlFor="login-username-input"
                className="flex items-center gap-1.5 text-xs font-medium"
              >
                <User className="h-3.5 w-3.5 text-muted-foreground" />
                {tx("用户名", "Username")}
              </label>
              <Input
                id="login-username-input"
                data-testid="login-username-input"
                type="text"
                autoComplete="username"
                placeholder="ops_admin"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                className="h-9"
              />
            </div>

            <div className="space-y-1.5">
              <label
                htmlFor="login-password-input"
                className="flex items-center gap-1.5 text-xs font-medium"
              >
                <Lock className="h-3.5 w-3.5 text-muted-foreground" />
                {tx("口令", "Password")}
              </label>
              <PasswordInput
                id="login-password-input"
                data-testid="login-password-input"
                autoComplete="current-password"
                placeholder="••••••••••••"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                className="h-9"
              />
            </div>

            {loginError && (
              <div
                role="alert"
                data-testid="login-error-banner"
                className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2.5 text-xs space-y-1"
              >
                <div className="font-mono-tech font-semibold text-red-600 dark:text-red-400">
                  {tx("认证失败", "Authentication Failed")} [{loginError.code}]
                </div>
                <div className="text-muted-foreground leading-relaxed">
                  {loginError.detail}
                </div>
              </div>
            )}

            <Button
              type="submit"
              size="lg"
              data-testid="login-submit-btn"
              className="w-full"
              disabled={submitting || !username.trim() || !password}
            >
              {submitting ? tx("登录中…", "Signing in…") : tx("登录", "Sign In")}
            </Button>
          </form>

          {authConfig.oidc_enabled && (
            <div className="space-y-3 pt-1">
              <div className="relative flex items-center justify-center">
                <div className="border-t border-border w-full" />
                <span className="bg-card px-2 text-[11px] text-muted-foreground shrink-0">
                  {tx("单点登录", "Single Sign-On")}
                </span>
                <div className="border-t border-border w-full" />
              </div>

              <Button
                type="button"
                variant="outline"
                size="lg"
                data-testid="oidc-login-btn"
                onClick={handleOidcLogin}
                className="w-full border-xblue/40 hover:border-xblue text-foreground"
              >
                <Globe className="h-3.5 w-3.5 text-xblue" />
                {authConfig.oidc_display_name || tx("企业单点登录", "Enterprise SSO")}
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
                {tx("访问令牌登录", "Personal Access Token")}
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
                    className="block text-xs font-medium"
                  >
                    {tx("访问令牌", "Access Token")}
                  </label>
                  <PasswordInput
                    id="access-token-input"
                    autoComplete="off"
                    placeholder="axp_••••••••••••••••"
                    value={tokenInput}
                    onChange={(e) => setTokenInput(e.target.value)}
                    className="font-mono-tech h-9"
                  />
                </div>

                {patError && (
                  <div
                    role="alert"
                    className="rounded-md border border-red-500/40 bg-red-500/10 px-2.5 py-2 text-xs text-red-600 dark:text-red-400"
                  >
                    {patError}
                  </div>
                )}

                <Button
                  type="submit"
                  variant="outline"
                  size="lg"
                  data-testid="pat-submit-btn"
                  className="w-full"
                  disabled={patSubmitting || !tokenInput.trim()}
                >
                  {patSubmitting
                    ? tx("校验中…", "Verifying…")
                    : tx("验证令牌", "Verify Token")}
                </Button>
              </form>
            )}
          </div>
        </Card>
      </main>
    </div>
  );
}
