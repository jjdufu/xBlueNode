import { useEffect, useState } from "react";
import { Moon, Sun } from "lucide-react";
import {
  createBrowserRouter,
  Navigate,
  NavLink,
  Outlet,
  RouterProvider,
  useLocation,
} from "react-router";
import { getAccessToken, setUnauthorizedHandler } from "@/lib/api";
import { cn } from "@/lib/utils";
import { MODULE_REGISTRY } from "@/modules/registry";
import { LoginPage } from "@/pages/LoginPage";

function ShellLayout() {
  const location = useLocation();
  const [dark, setDark] = useState(
    () =>
      typeof document !== "undefined" &&
      document.documentElement.classList.contains("dark")
  );

  const token = getAccessToken();
  if (!token) {
    return <Navigate to="/login" replace />;
  }

  const toggleTheme = () => {
    const next = !dark;
    setDark(next);
    document.documentElement.classList.toggle("dark", next);
  };

  const navItems = MODULE_REGISTRY.filter((m) => !m.public);

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
              const active =
                item.path === "/"
                  ? location.pathname === "/"
                  : location.pathname === item.path ||
                    location.pathname.startsWith(`${item.path}/`);
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
        <Outlet />
      </main>
    </div>
  );
}

const router = createBrowserRouter([
  {
    path: "/login",
    element: <LoginPage />,
  },
  {
    element: <ShellLayout />,
    children: [
      ...MODULE_REGISTRY.filter((m) => !m.public).map((m) => {
        const PageComponent = m.component;
        return {
          path: m.path,
          element: <PageComponent />,
        };
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
