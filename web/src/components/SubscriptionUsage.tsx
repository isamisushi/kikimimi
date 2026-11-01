import { t, intlLocale } from "../i18n";
import { Button, NativeSelect, Card } from "@kikimimi/ui";
import { useEffect, useState } from "react";
import { getSubscriptionUsage } from "../api/client";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "./QueryBoundary";

function duration(minutes: number | null): string {
  if (minutes === null) return t("Window duration unknown");
  if (minutes % 1440 === 0) return t("{0} day window", [minutes / 1440]);
  if (minutes % 60 === 0) return t("{0} hour window", [minutes / 60]);
  return t("{0} minute window", [minutes]);
}

export function SubscriptionUsage({
  compactEmpty = false,
}: {
  compactEmpty?: boolean;
}) {
  const state = useAsync(getSubscriptionUsage, []);
  const [now, setNow] = useState(Date.now());
  const [account, setAccount] = useState("");
  useEffect(() => {
    const timer = window.setInterval(() => {
      setNow(Date.now());
      state.reload();
    }, 60_000);
    return () => window.clearInterval(timer);
  }, [state.reload]);

  if (compactEmpty && state.status === "ok" && state.data.length === 0) {
    return (
      <details className="panel subscription-empty">
        <summary>{t("Subscription usage · no observations yet")}</summary>
        <p>
          {t(
            "Usage appears after it is recorded on this Mac. This is separate from session activity.",
          )}
        </p>
        <p>
          {t("Codex updates while the agent is running. To fetch now:")}{" "}
          <code>kikimimi usage codex</code>
        </p>
        <p>
          {t("For Claude, configure")}{" "}
          <code>kikimimi usage claude --account personal</code>{" "}
          {t("as your status-line command.")}
        </p>
        <Button variant="outline" type="button" onClick={state.reload}>
          {t("Refresh usage")}
        </Button>
      </details>
    );
  }

  return (
    <Card
      className="panel subscription-usage"
      aria-labelledby="subscription-usage-title"
    >
      <h2 className="panel__title" id="subscription-usage-title">
        {t("Subscription usage by account")}
      </h2>
      <p className="panel__note">
        {t(
          "Latest observations on this machine. Codex accounts are detected automatically. Refresh reloads recorded observations.",
        )}
      </p>
      <QueryBoundary state={state} onRetry={state.reload}>
        {(snapshots) =>
          snapshots.length === 0 ? (
            <div className="state-panel">
              <p>{t("No account usage recorded yet.")}</p>
              <p>
                {t(
                  "Codex updates automatically while the agent is running. To fetch now:",
                )}{" "}
                <code>kikimimi usage codex</code>
              </p>
              <p>
                {t("Claude: configure")}{" "}
                <code>kikimimi usage claude --account personal</code>{" "}
                {t("as your status-line command.")}
              </p>
              <p>{t("Use a separate Claude label for each subscription.")}</p>
            </div>
          ) : (
            <>
              <div className="subscription-toolbar">
                <label className="field">
                  {t("Account")}
                  <NativeSelect
                    value={account}
                    onChange={(event) => setAccount(event.target.value)}
                  >
                    <option value="">{t("All accounts")}</option>
                    {snapshots.map((s) => (
                      <option
                        key={`${s.agent}/${s.account}`}
                        value={JSON.stringify([s.agent, s.account])}
                      >
                        {s.agent === "claude" ? "Claude" : "Codex"} ·{" "}
                        {s.account}
                      </option>
                    ))}
                  </NativeSelect>
                </label>
                <Button
                  variant="outline"
                  type="button"
                  onClick={() => {
                    setNow(Date.now());
                    state.reload();
                  }}
                >
                  {t("Refresh")}
                </Button>
              </div>
              <div className="subscription-accounts">
                {snapshots
                  .filter(
                    (s) =>
                      !account ||
                      account === JSON.stringify([s.agent, s.account]),
                  )
                  .map((s) => {
                    const age = Math.max(
                      0,
                      Math.floor((now - s.observed_at) / 60_000),
                    );
                    return (
                      <article
                        className="subscription-account"
                        key={`${s.agent}/${s.account}`}
                      >
                        <h3>
                          {s.agent === "claude" ? "Claude" : "Codex"} ·{" "}
                          {s.account}
                        </h3>
                        <p className="panel__note">
                          <time
                            dateTime={new Date(s.observed_at).toISOString()}
                            title={new Date(s.observed_at).toLocaleString(
                              intlLocale(),
                            )}
                          >
                            {t("Observed")}{" "}
                            {age < 1 ? t("just now") : t("{0} min ago", [age])}
                          </time>
                          {age >= 15 ? t(" · May be outdated") : ""}
                        </p>
                        {s.windows.length === 0 && (
                          <p>{t("Usage unavailable")}</p>
                        )}
                        {s.windows.map((w) => {
                          const expired =
                            w.resets_at !== null && w.resets_at * 1000 <= now;
                          return (
                            <div className="subscription-window" key={w.name}>
                              <p>
                                {duration(w.window_minutes)} ·{" "}
                                {w.name.replace(/_/g, " ")}
                              </p>
                              <p>
                                {w.used_percent.toFixed(1)}
                                {t("% used")}
                                {expired ? t(" at last observation") : ""}
                              </p>
                              <progress
                                max={100}
                                value={w.used_percent}
                                aria-label={t("{0} {1} {2} usage", [
                                  s.agent,
                                  s.account,
                                  w.name,
                                ])}
                                style={{
                                  width: "100%",
                                  opacity: expired ? 0.45 : 1,
                                }}
                              />
                              <p className="panel__note">
                                {w.resets_at === null
                                  ? t("Reset time unknown")
                                  : expired
                                    ? t(
                                        "Reset time passed — awaiting a new observation",
                                      )
                                    : t("Resets {0}", [
                                        new Date(
                                          w.resets_at * 1000,
                                        ).toLocaleString(intlLocale()),
                                      ])}
                              </p>
                            </div>
                          );
                        })}
                      </article>
                    );
                  })}
              </div>
            </>
          )
        }
      </QueryBoundary>
    </Card>
  );
}
