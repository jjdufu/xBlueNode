import {
  type FormEvent,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  ChevronDown,
  KeyRound,
  Languages,
  LogOut,
  Moon,
  PanelLeftClose,
  PanelLeftOpen,
  Search,
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
import { CommandPalette } from "@/components/ui/CommandPalette";
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
import {
  LEGACY_ROUTE_REDIRECTS,
  MODULE_REGISTRY,
  type LegacyRedirectSpec,
  type ModuleRouteSpec,
} from "@/modules/registry";
import { LoginPage } from "@/pages/LoginPage";

function matchesModulePath(spec: ModuleRouteSpec, pathname: string): boolean {
  if (spec.path === "/") {
    return pathname === "/";
  }
  if (pathname === spec.path || pathname.startsWith(`${spec.path}/`)) {
    return true;
  }
  if (spec.activeMatchPrefixes) {
    return spec.activeMatchPrefixes.some(
      (prefix) => pathname === prefix || pathname.startsWith(`${prefix}/`)
    );
  }
  return false;
}

function LegacyRouteRedirect({ spec }: { readonly spec: LegacyRedirectSpec }) {
  const location = useLocation();
  const merged = new URLSearchParams(location.search);
  for (const [k, v] of Object.entries(spec.defaultQuery)) {
    if (!merged.has(k)) {
      merged.set(k, v);
    }
  }
  const qs = merged.toString();
  const target = qs ? `${spec.toPathname}?${qs}` : spec.toPathname;
  return <Navigate to={target} replace />;
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
  const [userSidebarOverride, setUserSidebarOverride] = useState<
    boolean | null
  >(null);
  const [cmdkOpen, setCmdkOpen] = useState(false);
  const [accountMenuOpen, setAccountMenuOpen] = useState(false);
  const accountMenuRef = useRef<HTMLDivElement | null>(null);

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

  useEffect(() => {
    setUserSidebarOverride(null);
    setAccountMenuOpen(false);
  }, [location.pathname]);

  useEffect(() => {
    if (!accountMenuOpen) return;
    const onDown = (ev: MouseEvent) => {
      if (
        accountMenuRef.current &&
        !accountMenuRef.current.contains(ev.target as Node)
      ) {
        setAccountMenuOpen(false);
      }
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") {
        setAccountMenuOpen(false);
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [accountMenuOpen]);

  if (!token && !bootRefreshDone) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background text-muted-foreground text-[13px]">
        {tx("会话恢复中…", "Restoring session…")}
      </div>
    );
  }

  if (!token) {
    return <Navigate to="/login" replace />;
  }

  const permittedModules = MODULE_REGISTRY.filter((m) => {
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

  const navItems = permittedModules.filter((m) => !m.sidebarHidden);
  const primaryNavItems = navItems.filter((m) => m.group !== "governance");
  const governanceNavItems = navItems.filter((m) => m.group === "governance");

  const activeModule = permittedModules.find(
    (m) => !m.sidebarHidden && matchesModulePath(m, location.pathname)
  ) ?? permittedModules.find((m) => matchesModulePath(m, location.pathname));

  const sidebarCollapsed =
    userSidebarOverride ?? Boolean(activeModule?.autoCollapseSidebar);

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
    setAccountMenuOpen(false);
    await logout();
    void navigate("/login", { replace: true });
  };

  const handleLogoutAll = async () => {
    setAccountMenuOpen(false);
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
        err instanceof ApiError
          ? `[${err.errorCode}] ${err.message}`
          : String(err)
      );
    } finally {
      setPwdSaving(false);
    }
  };

  const primaryRole = cachedMe?.roles?.[0] ?? "";
  const isFullBleed = Boolean(activeModule?.fullBleed);

  const renderSidebarLink = (item: ModuleRouteSpec) => {
    const Icon = item.icon;
    const active = matchesModulePath(item, location.pathname);
    const fullNavLabel = isZh ? item.label : item.enLabel;
    const coreLabel = isZh ? item.shortZh : item.shortEn;
    const navTestId =
      item.key === "governance"
        ? "sidebar-nav-settings"
        : `sidebar-nav-${item.key}`;
    return (
      <NavLink
        key={item.key}
        to={item.path}
        end={item.path === "/"}
        aria-label={fullNavLabel}
        title={fullNavLabel}
        data-testid={navTestId}
        className={cn(
          "relative w-full flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-[13px] transition-colors cursor-pointer",
          sidebarCollapsed && "justify-center px-0 py-2",
          active
            ? "bg-xblue-soft text-xblue font-semibold before:absolute before:left-0 before:top-1.5 before:bottom-1.5 before:w-[3px] before:rounded-r before:bg-xblue"
            : "font-medium text-muted-foreground hover:text-foreground hover:bg-muted/70"
        )}
      >
        <Icon className={cn("h-4 w-4 shrink-0", active && "text-xblue")} />
        {!sidebarCollapsed && <span className="truncate">{coreLabel}</span>}
      </NavLink>
    );
  };

  return (
    <div
      className={cn(
        "flex flex-col bg-background text-foreground",
        isFullBleed ? "h-screen overflow-hidden" : "min-h-screen"
      )}
    >
      {/* Global Sticky Top Header */}
      <header
        data-testid="app-top-header"
        className="sticky top-0 z-30 h-12 border-b border-border bg-card/95 backdrop-blur-xs flex items-center justify-between shrink-0"
      >
        {/* Top-Left Brand Zone: Strictly aligned with left sidebar width */}
        <div className="h-full flex items-center shrink-0">
          <div
            className={cn(
              "h-full flex items-center justify-between border-r border-border transition-all shrink-0",
              sidebarCollapsed ? "w-14 px-2 justify-center" : "w-52 px-3.5"
            )}
          >
            {!sidebarCollapsed && (
              <div className="flex items-center gap-2 min-w-0 select-none">
                <span className="inline-flex h-6 w-6 items-center justify-center rounded-md bg-xblue text-white text-xs font-bold tracking-tight shrink-0">
                  xB
                </span>
                <span className="text-[15px] font-semibold tracking-tight text-foreground truncate">
                  xBlueNode
                </span>
              </div>
            )}

            <IconButton
              size="sm"
              onClick={() => setUserSidebarOverride(!sidebarCollapsed)}
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
          </div>

          {sidebarCollapsed && (
            <div className="flex items-center gap-2 pl-3 select-none">
              <span className="inline-flex h-6 w-6 items-center justify-center rounded-md bg-xblue text-white text-xs font-bold tracking-tight shrink-0">
                xB
              </span>
              <span className="text-[15px] font-semibold tracking-tight text-foreground">
                xBlueNode
              </span>
            </div>
          )}
        </div>

        {/* Top-Right: Command Palette Trigger + Preferences + Unified Account Dropdown */}
        <div className="flex items-center gap-1.5 px-3 md:px-4 ml-auto shrink-0">
          <button
            type="button"
            data-testid="global-cmdk-trigger"
            onClick={() => setCmdkOpen(true)}
            aria-label={tx("打开命令面板", "Open command palette")}
            className="inline-flex h-8 items-center gap-2 rounded-lg border border-border/80 bg-muted/40 px-2.5 text-xs text-muted-foreground hover:text-foreground hover:bg-muted transition-colors cursor-pointer"
          >
            <Search className="h-3.5 w-3.5 text-xblue shrink-0" />
            <span className="hidden sm:inline">
              {tx("搜索视图、模型与图表…", "Search views, models, charts…")}
            </span>
            <kbd className="rounded border border-border bg-card px-1.5 py-0.5 text-[10px] font-mono-tech text-muted-foreground">
              ⌘K
            </kbd>
          </button>

          <button
            type="button"
            data-testid="locale-toggle-btn"
            onClick={toggleLocale}
            aria-label={tx("切换语言", "Switch language")}
            title={tx("切换为 English", "Switch to 简体中文")}
            className="inline-flex h-8 items-center gap-1 rounded-md px-2 text-xs font-medium text-muted-foreground hover:text-foreground hover:bg-muted transition-colors shrink-0 cursor-pointer"
          >
            <Languages className="h-3.5 w-3.5 text-xblue" />
            <span>{isZh ? "中文" : "EN"}</span>
          </button>

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

          {/* Unified User Account Dropdown */}
          <div ref={accountMenuRef} className="relative">
            <button
              type="button"
              data-testid="shell-user-menu-btn"
              aria-expanded={accountMenuOpen}
              aria-haspopup="menu"
              onClick={() => setAccountMenuOpen((v) => !v)}
              className="inline-flex h-8 items-center gap-1.5 rounded-md px-2 text-xs hover:bg-muted transition-colors cursor-pointer"
            >
              <UserCheck className="h-3.5 w-3.5 text-xblue shrink-0" />
              <span
                data-testid="shell-user-badge"
                className="inline-flex items-center gap-1.5"
              >
                <span className="font-medium">
                  {cachedMe?.username ?? tx("账号", "Account")}
                </span>
                {cachedMe?.dept_id && (
                  <span className="hidden sm:inline text-muted-foreground">
                    · {cachedMe.dept_id}
                  </span>
                )}
                {primaryRole && (
                  <Badge
                    variant="xblue"
                    className="hidden md:inline-flex ml-0.5 text-[11px]"
                  >
                    {tEnum(primaryRole)}
                  </Badge>
                )}
              </span>
              <ChevronDown
                className={cn(
                  "h-3.5 w-3.5 text-muted-foreground transition-transform",
                  accountMenuOpen && "rotate-180"
                )}
              />
            </button>

            {accountMenuOpen && (
              <div
                role="menu"
                className="absolute right-0 top-full mt-1.5 w-52 rounded-lg border border-border bg-card text-card-foreground shadow-lg py-1 z-50"
              >
                {cachedMe && (
                  <div className="px-3 py-2 border-b border-border space-y-0.5">
                    <div className="text-xs font-semibold truncate">
                      {cachedMe.username}
                    </div>
                    <div className="flex items-center gap-1.5 text-[11px] text-muted-foreground">
                      <span>{cachedMe.dept_id}</span>
                      {primaryRole && (
                        <>
                          <span>·</span>
                          <span>{tEnum(primaryRole)}</span>
                        </>
                      )}
                    </div>
                  </div>
                )}

                <div className="p-1 space-y-0.5">
                  <button
                    type="button"
                    role="menuitem"
                    data-testid="shell-change-password-btn"
                    onClick={() => {
                      setAccountMenuOpen(false);
                      setPwdError(null);
                      setPwdModalOpen(true);
                    }}
                    className="w-full flex items-center gap-2 rounded px-2.5 py-1.5 text-xs text-foreground hover:bg-muted transition-colors cursor-pointer"
                  >
                    <KeyRound className="h-3.5 w-3.5 text-muted-foreground" />
                    <span>{tx("修改口令", "Change password")}</span>
                  </button>

                  <button
                    type="button"
                    role="menuitem"
                    data-testid="shell-logout-all-btn"
                    onClick={() => void handleLogoutAll()}
                    className="w-full flex items-center gap-2 rounded px-2.5 py-1.5 text-xs text-foreground hover:bg-muted transition-colors cursor-pointer"
                  >
                    <ShieldOff className="h-3.5 w-3.5 text-muted-foreground" />
                    <span>{tx("退出全部设备", "Sign out all devices")}</span>
                  </button>
                </div>

                <div className="border-t border-border p-1">
                  <button
                    type="button"
                    role="menuitem"
                    data-testid="shell-logout-btn"
                    onClick={() => void handleLogout()}
                    className="w-full flex items-center gap-2 rounded px-2.5 py-1.5 text-xs text-red-600 dark:text-red-400 hover:bg-red-500/10 transition-colors cursor-pointer"
                  >
                    <LogOut className="h-3.5 w-3.5" />
                    <span>{tx("退出登录", "Sign out")}</span>
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      </header>

      {/* Main Workspace Body */}
      <div
        className={cn(
          "flex flex-1 min-h-0",
          isFullBleed && "overflow-hidden"
        )}
      >
        <aside
          data-testid="app-sidebar"
          className={cn(
            "sticky top-12 h-[calc(100vh-3rem)] border-r border-border bg-card/35 flex flex-col justify-between shrink-0 transition-all overflow-y-auto",
            sidebarCollapsed ? "w-14 px-2 py-2.5" : "w-52 px-2.5 py-3"
          )}
        >
          <nav
            data-testid="primary-domain-nav"
            className="space-y-0.5"
          >
            {primaryNavItems.map((item) => renderSidebarLink(item))}
          </nav>

          <div className="mt-3 space-y-2">
            {governanceNavItems.length > 0 && (
              <div
                data-testid="governance-domain-section"
                className="pt-2 border-t border-border/60 space-y-0.5"
              >
                {governanceNavItems.map((item) => renderSidebarLink(item))}
              </div>
            )}

            {/* Subtle version anchor at sidebar bottom */}
            <div
              className={cn(
                "pt-2 border-t border-border/60 text-[11px] text-muted-foreground/70 select-none",
                sidebarCollapsed
                  ? "text-center font-mono-tech"
                  : "px-2.5 flex items-center justify-between"
              )}
            >
              {sidebarCollapsed ? (
                <span>v{__APP_VERSION__}</span>
              ) : (
                <>
                  <span>xBlueNode</span>
                  <span className="font-mono-tech">v{__APP_VERSION__}</span>
                </>
              )}
            </div>
          </div>
        </aside>

        <main
          className={cn(
            "flex-1 min-w-0 w-full",
            isFullBleed
              ? "h-[calc(100vh-3rem)] overflow-hidden flex flex-col p-2.5 md:p-3"
              : "max-w-[1760px] mx-auto p-4 md:p-6"
          )}
        >
          <Outlet />
        </main>
      </div>

      <CommandPalette open={cmdkOpen} onOpenChange={setCmdkOpen} />

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
      ...LEGACY_ROUTE_REDIRECTS.map((spec) => ({
        path: spec.from,
        element: <LegacyRouteRedirect spec={spec} />,
      })),
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
