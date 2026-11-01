import { t } from "../i18n";
import { minutesSince } from "../api/format";

/** Traffic-light freshness indicator for "last seen" timestamps. */
export function FreshnessBadge({
  lastEventTs,
}: {
  lastEventTs: string | null;
}) {
  const mins = minutesSince(lastEventTs);

  if (mins === null) {
    return (
      <span className="freshness freshness--unknown">
        <span className="freshness__dot" aria-hidden="true" />
        {t("Unknown")}
      </span>
    );
  }

  let tone: "live" | "recent" | "stale";
  let text: string;
  if (mins <= 15) {
    tone = "live";
    text = t("Active");
  } else if (mins < 60) {
    tone = "recent";
    text = t("{0}m ago", [mins]);
  } else if (mins < 60 * 24) {
    tone = "recent";
    text = t("{0}h ago", [Math.floor(mins / 60)]);
  } else {
    tone = "stale";
    text = t("{0}d ago", [Math.floor(mins / (60 * 24))]);
  }

  return (
    <span className={`freshness freshness--${tone}`}>
      <span className="freshness__dot" aria-hidden="true" />
      {text}
    </span>
  );
}
