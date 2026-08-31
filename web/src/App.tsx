import { RouterProvider, useRouter } from "./router/Router";
import { SessionProvider, useSession } from "./hooks/useSession";
import { Layout } from "./components/Layout";
import { Login } from "./routes/Login";
import { Overview } from "./routes/Overview";
import { Tools } from "./routes/Tools";
import { Mcp } from "./routes/Mcp";
import { Sessions } from "./routes/Sessions";

function AppRoutes() {
  const { path } = useRouter();
  const { status } = useSession();

  if (status === "loading") {
    return (
      <div className="boot-loading" role="status">
        <span className="spinner" aria-hidden="true" />
        Loading kikimimi…
      </div>
    );
  }

  if (status === "anon") {
    // useSession's effect redirects here; render immediately to avoid a flash
    // of the wrong screen while history.pushState settles.
    return <Login />;
  }

  if (path === "/login") {
    // Authenticated but still on /login for a tick; useSession's effect is
    // about to redirect to "/". Avoid flashing NotFound in the meantime.
    return null;
  }

  let page;
  switch (path) {
    case "/":
      page = <Overview />;
      break;
    case "/tools":
      page = <Tools />;
      break;
    case "/mcp":
      page = <Mcp />;
      break;
    case "/sessions":
      page = <Sessions />;
      break;
    default:
      page = <NotFound />;
  }

  return <Layout>{page}</Layout>;
}

function NotFound() {
  return (
    <div className="page">
      <div className="state-panel state-panel--empty">Page not found</div>
    </div>
  );
}

export function App() {
  return (
    <RouterProvider>
      <SessionProvider>
        <AppRoutes />
      </SessionProvider>
    </RouterProvider>
  );
}
