import { t } from "../i18n";
import { useSession } from "../hooks/useSession";
import { Link } from "../router/Router";

declare global {
  interface Window {
    __kikimimiDesktop?: boolean;
  }
}

export function OverviewEmptyState({ onRefresh }: { onRefresh: () => void }) {
  const { session } = useSession();
  const local = session?.local && session.data_source !== "s3";
  const desktop = local && window.__kikimimiDesktop;
  return (
    <section
      className="panel overview-empty"
      aria-labelledby="overview-empty-title"
    >
      <p className="overview-empty__eyebrow">{t("GET STARTED")}</p>
      <h2 id="overview-empty-title">{t("No activity in the last 14 days")}</h2>
      <p>
        {local
          ? t(
              "Bring your Claude Code and Codex activity into one place. Already collecting? Run a new agent session, then refresh this page.",
            )
          : t(
              "This workspace has no activity for this period. Check the data source and where your machines send activity.",
            )}
      </p>
      {local && (
        <ol className="overview-empty__steps">
          <li>
            <strong>{t("Review collection")}</strong>
            <span>
              {t(
                "Set up collection on this Mac or connect Cloud sync to bring in activity from your other devices.",
              )}
            </span>
          </li>
          <li>
            <strong>{t("Use your agent")}</strong>
            <span>
              {t("After setup, start a new Claude Code or Codex session.")}
            </span>
          </li>
          <li>
            <strong>{t("See your activity")}</strong>
            <span>
              {t("Refresh to see recorded sessions, tools, and usage.")}
            </span>
          </li>
        </ol>
      )}
      <div className="overview-empty__actions">
        {desktop ? (
          <a className="btn btn--primary" href="/__desktop_setup">
            {t("Review collection setup…")}
          </a>
        ) : (
          <Link className="btn btn--primary" to="/storage">
            {local ? t("View setup options") : t("Check data source")}
          </Link>
        )}
        <button type="button" className="btn btn--ghost" onClick={onRefresh}>
          {t("Refresh activity")}
        </button>
      </div>
      {desktop && (
        <p className="overview-empty__note">
          {t("Opening setup does not start collection or change sharing.")}
        </p>
      )}
    </section>
  );
}
