import { type FormEvent, useEffect, useState, useSyncExternalStore } from "react";
import { KeyRound, LogOut, Moon, ShieldOff, Sun, X } from "lucide-react";
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
  getAccessToken,
  getCachedMe,
  logout,
  logoutAll,
  refreshAccessToken,
  setUnauthorizedHandler,
  subscribeAuthChange,
} from "@/lib/api";
import { validateLocalPassword } from "@/components/axguard/logic";
import { Button, Card, Input } from "@/components/ui/primitives";
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
  const [dark, setDark] = useState(
    () =>
      typeof document !== "undefined" &&
      document.documentElement.classList.contains("dark")
  );

  // Password change modal state
  const [pwdModalOpen, setPwdModalOpen] = useState(false);
  const [oldPassword, setOldPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [pwdSaving, setPwdSaving] = useState(false);
  const [pwdError, setPwdError] = useState<string | null>(null);

  useEffect(() => {
    if (token !== null) {
      setBootRefreshDone(true);
      return;
    }
    if (bootRefreshDone) {
      return;
    }
    let cancelled = false;
    void refreshAccessToken().finally(() => {
      if (!cancelled) {
        setBootRefreshDone(true);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [token, bootRefreshDone]);

  if (!token && !bootRefreshDone) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background text-muted-foreground text-xs font-mono-tech">
        正在恢复安全会话...
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

  const toggleTheme = () => {
    const next = !dark;
    setDark(next);
    document.documentElement.classList.toggle("dark", next);
  };

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
            {navItems.map((item) => {
              const Icon = item.icon;
              const active = matchesModulePath(item, location.pathname);
              return (
                <NavLink
                  key={item.key}
                  to={item.path}
                  end={item.path === "/"}
                  className={cn(
                    "w-full flex items-center gap-2.5 rounded-md px-2.5 py-2 text-xs font-medium transition-colors cursor-pointer",
                    active
                      ? "bg-muted text-foreground border border-border"
                      : "text-muted-foreground hover:text-foreground hover:bg-muted/50"
                  )}
                >
                  <Icon className={cn("h-3.5 w-3.5", active && "text-xblue")} />
                  {item.label}
                </NavLink>
              );
            })}
          </nav>
        </div>

        <div className="space-y-2 pt-4 border-t border-border">
          <div className="grid grid-cols-3 gap-1">
            <button
              type="button"
              onClick={() => {
                setPwdError(null);
                setPwdModalOpen(true);
              }}
              data-testid="shell-change-password-btn"
              title="修改登录口令"
              className="flex items-center justify-center gap-1 rounded border border-border px-1.5 py-1.5 text-[11px] text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
            >
              <KeyRound className="h-3 w-3" />
              口令
            </button>
            <button
              type="button"
              onClick={() => void handleLogout()}
              data-testid="shell-logout-btn"
              title="退出当前会话"
              className="flex items-center justify-center gap-1 rounded border border-border px-1.5 py-1.5 text-[11px] text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
            >
              <LogOut className="h-3 w-3" />
              退出
            </button>
            <button
              type="button"
              onClick={() => void handleLogoutAll()}
              data-testid="shell-logout-all-btn"
              title="吊销该账号全部活跃会话"
              className="flex items-center justify-center gap-1 rounded border border-border px-1.5 py-1.5 text-[11px] text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
            >
              <ShieldOff className="h-3 w-3" />
              全退
            </button>
          </div>

          <button
            type="button"
            onClick={toggleTheme}
            className="w-full flex items-center gap-2 rounded-md border border-border px-2.5 py-2 text-xs text-muted-foreground hover:text-foreground cursor-pointer"
          >
            {dark ? <Sun className="h-3.5 w-3.5" /> : <Moon className="h-3.5 w-3.5" />}
            {dark ? "浅色极简模式" : "深色极简模式"}
          </button>
        </div>
      </aside>

      <main
        className={cn(
          "flex-1 min-w-0 w-full mx-auto",
          activeModule?.wideLayout
            ? "p-4 md:p-6 max-w-[1600px]"
            : "p-6 md:p-8 max-w-6xl"
        )}
      >
        <Outlet />
      </main>

      {pwdModalOpen && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
          <Card className="w-full max-w-md p-5 space-y-4">
            <div className="flex items-center justify-between border-b border-border pb-3">
              <div className="text-sm font-semibold flex items-center gap-2">
                <KeyRound className="h-4 w-4 text-xblue" />
                修改当前账号登录口令
              </div>
              <button
                type="button"
                onClick={() => setPwdModalOpen(false)}
                className="text-muted-foreground hover:text-foreground cursor-pointer"
              >
                <X className="h-4 w-4" />
              </button>
            </div>

            <form
              onSubmit={(e) => void handlePasswordSubmit(e)}
              className="space-y-3"
            >
              <div className="space-y-1">
                <label className="text-xs font-medium">当前旧口令 *</label>
                <Input
                  required
                  type="password"
                  value={oldPassword}
                  onChange={(e) => setOldPassword(e.target.value)}
                  className="h-8 text-xs"
                />
              </div>
              <div className="space-y-1">
                <label className="text-xs font-medium">
                  新口令（≥ 12 位且不含用户名）*
                </label>
                <Input
                  required
                  type="password"
                  value={newPassword}
                  onChange={(e) => setNewPassword(e.target.value)}
                  className="h-8 text-xs"
                />
              </div>

              {pwdError && (
                <div
                  role="alert"
                  className="rounded border border-foreground/30 bg-muted px-3 py-2 text-xs"
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
                  取消
                </Button>
                <Button
                  type="submit"
                  disabled={pwdSaving || !oldPassword || !newPassword}
                >
                  {pwdSaving ? "提交中..." : "确认修改并重新登录"}
                </Button>
              </div>
            </form>
          </Card>
        </div>
      )}
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
