import { type FormEvent, useEffect, useState, useSyncExternalStore } from "react";
import {
  KeyRound,
  Languages,
  LogOut,
  Moon,
  PanelLeftClose,
  PanelLeftOpen,
  ShieldOff,
  Sun,
  UserCheck,
} from "lucide-react";
import {
  createBrowserRouter,
  Navigate,
  NavLink,
  Outlet,
  RouterProvider,
  useLocation,
  useNavigate,
} from "react-router";
import {
  ApiError,
  changePassword,
  clearAccessToken,
  fetchMe,
  getAccessToken,
  getCachedMe,
  logout,
  logoutAll,
  refreshAccessToken,
  setUnauthorizedHandler,
  subscribeAuthChange,
} from "@/lib/api";
import { validateLocalPassword } from "@/components/axguard/logic";
import {
  Badge,
  Button,
  IconButton,
  Modal,
  PasswordInput,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { useTheme } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { MODULE_REGISTRY, type ModuleRouteSpec } from "@/modules/registry";
import { LoginPage } from "@/pages/LoginPage";

function matchesModulePath(spec: ModuleRouteSpec, pathname: string): boolean {
  if (spec.path === "/") {
    return pathname === "/";
  }
  return pathname === spec.path || pathname.startsWith(`${spec.path}/`);
}

function ShellLayout() {
  const location = useLocation();
  const navigate = useNavigate();
  const { isDark, toggleTheme } = useTheme();
  const { isZh, toggleLocale, tx, tEnum } = useI18n();

  const token = useSyncExternalStore(
    subscribeAuthChange,
    getAccessToken,
    () => null
  );
  const cachedMe = useSyncExternalStore(
    subscribeAuthChange,
    getCachedMe,
    () => null
  );

  const [bootRefreshDone, setBootRefreshDone] = useState(
    () => getAccessToken() !== null
  );
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);

  // Password change modal state
  const [pwdModalOpen, setPwdModalOpen] = useState(false);
  const [oldPassword, setOldPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [pwdSaving, setPwdSaving] = useState(false);
  const [pwdError, setPwdError] = useState<string | null>(null);

  useEffect(() => {
    if (token !== null) {
      setBootRefreshDone(true);
      if (!cachedMe) {
        void fetchMe().catch(() => null);
      }
      return;
    }
    if (bootRefreshDone) {
      return;
    }
    let cancelled = false;
    void refreshAccessToken()
      .then(async (refreshed) => {
        if (refreshed && !cancelled) {
          await fetchMe().catch(() => null);
        }
      })
      .finally(() => {
        if (!cancelled) {
          setBootRefreshDone(true);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [token, bootRefreshDone, cachedMe]);

  if (!token && !bootRefreshDone) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background text-muted-foreground text-xs font-mono-tech">
        {tx("会话恢复中…", "Restoring session…")}
      </div>
    );
  }

  if (!token) {
    return <Navigate to="/login" replace />;
  }

  const navItems = MODULE_REGISTRY.filter((m) => {
    if (m.public) {
      return false;
    }
    if (
      m.requiredAnyPermissions &&
      cachedMe &&
      Array.isArray(cachedMe.permissions)
    ) {
      if (cachedMe.roles?.includes("ADMIN")) {
        return true;
      }
      const userPerms = cachedMe.permissions as readonly string[];
      return m.requiredAnyPermissions.some((req) => userPerms.includes(req));
    }
    return true;
  });

  const activeModule = navItems.find((m) =>
    matchesModulePath(m, location.pathname)
  );
  const searchParams = new URLSearchParams(location.search);
  const isImmersive = Boolean(
    activeModule?.immersiveQueryParam &&
      location.pathname !== activeModule.path &&
      searchParams.get(activeModule.immersiveQueryParam) === "1"
  );

  if (isImmersive) {
    return <Outlet />;
  }

  const handleLogout = async () => {
    await logout();
    void navigate("/login", { replace: true });
  };

  const handleLogoutAll = async () => {
    try {
      await logoutAll();
    } catch {
      clearAccessToken();
    }
    void navigate("/login", { replace: true });
  };

  const handlePasswordSubmit = async (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (newPassword !== confirmPassword) {
      setPwdError(
        tx("两次输入的新口令不一致", "New passwords do not match")
      );
      return;
    }
    const issue = validateLocalPassword(newPassword, cachedMe?.username);
    if (issue) {
      setPwdError(issue);
      return;
    }
    setPwdSaving(true);
    setPwdError(null);
    try {
      await changePassword(oldPassword, newPassword);
      setPwdModalOpen(false);
      setOldPassword("");
      setNewPassword("");
      setConfirmPassword("");
      clearAccessToken();
      void navigate("/login", { replace: true });
    } catch (err) {
      setPwdError(
        err instanceof ApiError ? `[${err.errorCode}] ${err.message}` : String(err)
      );
    } finally {
      setPwdSaving(false);
    }
  };

  const primaryRole = cachedMe?.roles?.[0] ?? "";

  return (
    <div className="min-h-screen flex flex-col bg-background text-foreground">
      {/* Global Sticky Top Header */}
      <header
        data-testid="app-top-header"
        className="sticky top-0 z-30 h-12 border-b border-border bg-card/95 backdrop-blur-xs px-3 md:px-4 flex items-center justify-between gap-3 shrink-0"
      >
        {/* Top-Left: Sidebar Toggle + Brand + Language Switcher */}
        <div className="flex items-center gap-2 min-w-0">
          <IconButton
            size="sm"
            onClick={() => setSidebarCollapsed((v) => !v)}
            aria-label={
              sidebarCollapsed
                ? tx("展开导航栏", "Expand sidebar")
                : tx("收起导航栏", "Collapse sidebar")
            }
            title={
              sidebarCollapsed
                ? tx("展开导航栏", "Expand sidebar")
                : tx("收起导航栏", "Collapse sidebar")
            }
          >
            {sidebarCollapsed ? (
              <PanelLeftOpen className="h-4 w-4" />
            ) : (
              <PanelLeftClose className="h-4 w-4" />
            )}
          </IconButton>

          <div className="flex items-center gap-1.5 pr-1 select-none">
            <span className="inline-block h-2 w-2 rounded-full bg-xblue" />
            <span className="text-sm font-semibold tracking-tight">
              xBlueNode
            </span>
            <span className="hidden sm:inline text-[11px] font-mono-tech text-muted-foreground">
              v{__APP_VERSION__}
            </span>
          </div>

          <button
            type="button"
            data-testid="locale-toggle-btn"
            onClick={toggleLocale}
            aria-label={tx("切换语言", "Switch language")}
            title={tx("切换为 English", "Switch to 简体中文")}
            className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] font-medium text-muted-foreground hover:text-foreground hover:bg-muted transition-colors shrink-0 cursor-pointer"
          >
            <Languages className="h-3.5 w-3.5 text-xblue" />
            <span className="font-mono-tech">{isZh ? "中文" : "EN"}</span>
          </button>
        </div>

        {/* Top-Right: User Identity + Password + Logout + Borderless Icon-Only Theme Toggle */}
        <div className="flex items-center gap-1.5 shrink-0">
          {cachedMe && (
            <div
              data-testid="shell-user-badge"
              className="hidden md:inline-flex items-center gap-1.5 rounded-md bg-muted/60 px-2.5 py-1 text-xs"
            >
              <UserCheck className="h-3.5 w-3.5 text-xblue shrink-0" />
              <span className="font-medium">{cachedMe.username}</span>
              <span className="text-muted-foreground">·</span>
              <span className="text-muted-foreground font-mono-tech">
                {cachedMe.dept_id}
              </span>
              {primaryRole && (
                <Badge variant="xblue" className="ml-0.5 text-[10px]">
                  {tEnum(primaryRole)}
                </Badge>
              )}
            </div>
          )}

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

          <div className="h-4 w-px bg-border mx-0.5" />

          <IconButton
            data-testid="shell-change-password-btn"
            onClick={() => {
              setPwdError(null);
              setPwdModalOpen(true);
            }}
            aria-label={tx("修改口令", "Change password")}
            title={tx("修改口令", "Change password")}
          >
            <KeyRound className="h-4 w-4" />
          </IconButton>

          <IconButton
            data-testid="shell-logout-all-btn"
            onClick={() => void handleLogoutAll()}
            aria-label={tx("退出全部设备", "Sign out all devices")}
            title={tx("退出全部设备", "Sign out all devices")}
          >
            <ShieldOff className="h-4 w-4" />
          </IconButton>

          <Button
            variant="ghost"
            size="sm"
            type="button"
            data-testid="shell-logout-btn"
            onClick={() => void handleLogout()}
            title={tx("退出登录", "Sign out")}
            className="px-2 text-muted-foreground hover:text-foreground"
          >
            <LogOut className="h-3.5 w-3.5" />
            <span className="hidden sm:inline">{tx("退出", "Sign out")}</span>
          </Button>
        </div>
      </header>

      {/* Main Workspace Body */}
      <div className="flex flex-1 min-h-0">
        <aside
          className={cn(
            "sticky top-12 h-[calc(100vh-3rem)] border-r border-border bg-card/40 flex flex-col shrink-0 transition-all overflow-y-auto",
            sidebarCollapsed ? "w-14 p-2" : "w-54 p-3"
          )}
        >
          <nav className="space-y-0.5">
            {navItems.map((item) => {
              const Icon = item.icon;
              const active = matchesModulePath(item, location.pathname);
              const navLabel = isZh ? item.label : item.enLabel;
              return (
                <NavLink
                  key={item.key}
                  to={item.path}
                  end={item.path === "/"}
                  title={navLabel}
                  className={cn(
                    "w-full flex items-center gap-2.5 rounded-md px-2.5 py-2 text-xs font-medium transition-colors cursor-pointer",
                    sidebarCollapsed && "justify-center px-0",
                    active
                      ? "bg-xblue-soft/70 text-xblue border border-xblue/25"
                      : "text-muted-foreground hover:text-foreground hover:bg-muted/60 border border-transparent"
                  )}
                >
                  <Icon
                    className={cn("h-4 w-4 shrink-0", active && "text-xblue")}
                  />
                  {!sidebarCollapsed && (
                    <span className="truncate">{navLabel}</span>
                  )}
                </NavLink>
              );
            })}
          </nav>
        </aside>

        <main className="flex-1 min-w-0 w-full max-w-[1760px] mx-auto p-4 md:p-6">
          <Outlet />
        </main>
      </div>

      {/* Password Change Modal */}
      <Modal
        open={pwdModalOpen}
        onClose={() => setPwdModalOpen(false)}
        maxWidthClass="max-w-md"
        title={
          <>
            <KeyRound className="h-4 w-4 text-xblue shrink-0" />
            <span>{tx("修改口令", "Change Password")}</span>
          </>
        }
      >
        <form
          onSubmit={(e) => void handlePasswordSubmit(e)}
          className="space-y-3"
        >
          <div className="space-y-1">
            <label className="text-xs font-medium">
              {tx("原口令", "Current password")} *
            </label>
            <PasswordInput
              required
              autoComplete="current-password"
              value={oldPassword}
              onChange={(e) => setOldPassword(e.target.value)}
            />
          </div>
          <div className="space-y-1">
            <label className="text-xs font-medium">
              {tx("新口令（至少 12 位）", "New password (min 12 chars)")} *
            </label>
            <PasswordInput
              required
              autoComplete="new-password"
              value={newPassword}
              onChange={(e) => setNewPassword(e.target.value)}
            />
          </div>
          <div className="space-y-1">
            <label className="text-xs font-medium">
              {tx("确认新口令", "Confirm new password")} *
            </label>
            <PasswordInput
              required
              autoComplete="new-password"
              value={confirmPassword}
              onChange={(e) => setConfirmPassword(e.target.value)}
            />
          </div>

          {pwdError && (
            <div
              role="alert"
              className="rounded border border-red-500/40 bg-red-500/10 px-3 py-2 text-xs text-red-600 dark:text-red-400"
            >
              {pwdError}
            </div>
          )}

          <div className="flex justify-end gap-2 pt-2 border-t border-border">
            <Button
              variant="outline"
              type="button"
              onClick={() => setPwdModalOpen(false)}
            >
              {tx("取消", "Cancel")}
            </Button>
            <Button
              type="submit"
              disabled={
                pwdSaving || !oldPassword || !newPassword || !confirmPassword
              }
            >
              {pwdSaving ? tx("保存中…", "Saving…") : tx("保存", "Save")}
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
}

const router = createBrowserRouter([
  {
    path: "/login",
    element: <LoginPage />,
  },
  ...MODULE_REGISTRY.filter((m) => m.public).flatMap((m) => {
    const PublicPageComponent = m.component;
    const allPaths = [m.path, ...(m.subPaths ?? [])];
    return allPaths.map((p) => ({
      path: p,
      element: <PublicPageComponent />,
    }));
  }),
  {
    element: <ShellLayout />,
    children: [
      ...MODULE_REGISTRY.filter((m) => !m.public).flatMap((m) => {
        const PageComponent = m.component;
        const allPaths = [m.path, ...(m.subPaths ?? [])];
        return allPaths.map((p) => ({
          path: p,
          element: <PageComponent />,
        }));
      }),
      {
        path: "*",
        element: <Navigate to="/" replace />,
      },
    ],
  },
]);

export function App() {
  useEffect(() => {
    setUnauthorizedHandler(() => {
      void router.navigate("/login", { replace: true });
    });
    return () => {
      setUnauthorizedHandler(null);
    };
  }, []);

  return <RouterProvider router={router} />;
}
