import { useLocale } from "./i18n";
import { Settings } from "./routes/Settings";
import { t } from "./i18n";
import { RouterProvider, useRouter } from "./router/Router";
import { SessionProvider, useSession } from "./hooks/useSession";
import { Layout } from "./components/Layout";
import { Overview } from "./routes/Overview";
import { Models } from "./routes/Models";
import { Tools } from "./routes/Tools";
import { Mcp } from "./routes/Mcp";
import { Skills } from "./routes/Skills";
import { Sessions } from "./routes/Sessions";
import { SessionDetail } from "./routes/SessionDetail";
import { Patterns } from "./routes/Patterns";
import { Subagents } from "./routes/Subagents";
import { Storage } from "./routes/Storage";

function AppRoutes() {
  const { path } = useRouter();

  const { status } = useSession();

  if (status === "loading") {
    return (
      <div className="boot-loading" role="status">
        <span className="spinner" aria-hidden="true" />
        {t("Loading kikimimi…")}
      </div>
    );
  }

  if (status === "error") {
    return (
      <div className="page">
        <div className="state-panel state-panel--error" role="alert">
          <h1>{t("Local dashboard unavailable")}</h1>
          <p>
            {t("Open the dashboard from the desktop app or run")}{" "}
            <code>kikimimi web</code>.
          </p>
          <button className="btn" onClick={() => window.location.reload()}>
            {t("Try again")}
          </button>
        </div>
      </div>
    );
  }

  if (path === "/settings")
    return (
      <Layout>
        <Settings />
      </Layout>
    );

  if (path.startsWith("/sessions/")) {
    const id = decodeURIComponent(path.slice("/sessions/".length));
    return (
      <Layout>
        <SessionDetail sessionId={id} />
      </Layout>
    );
  }

  let page;
  switch (path) {
    case "/storage":
      page = <Storage />;
      break;
    case "/":
    case "/overview":
      page = <Overview />;
      break;
    case "/models":
      page = <Models />;
      break;
    case "/tools":
      page = <Tools />;
      break;
    case "/mcp":
      page = <Mcp />;
      break;
    case "/skills":
      page = <Skills />;
      break;
    case "/sessions":
      page = <Sessions />;
      break;
    case "/patterns":
      page = <Patterns />;
      break;
    case "/subagents":
      page = <Subagents />;
      break;
    default:
      page = <NotFound />;
  }

  return <Layout>{page}</Layout>;
}

function NotFound() {
  return (
    <div className="page">
      <div className="state-panel state-panel--empty">
        {t("Page not found")}
      </div>
    </div>
  );
}

export function App({
  embedded = false,
  onStorage,
  onNavigate,
}: {
  embedded?: boolean;
  onStorage?: () => void;
  onNavigate?: () => void;
}) {
  useLocale();
  return (
    <RouterProvider
      memory={embedded}
      onStorage={onStorage}
      onNavigate={onNavigate}
    >
      <SessionProvider>
        <AppRoutes />
      </SessionProvider>
    </RouterProvider>
  );
}
