import { minutesSince } from "../api/format";

/** Traffic-light freshness indicator for "last seen" timestamps. */
export function FreshnessBadge({ lastEventTs }: { lastEventTs: string | null }) {
  const mins = minutesSince(lastEventTs);

  if (mins === null) {
    return (
      <span className="freshness freshness--unknown">
        <span className="freshness__dot" aria-hidden="true" />
        Unknown
      </span>
    );
  }

  let tone: "live" | "recent" | "stale";
  let text: string;
  if (mins <= 15) {
    tone = "live";
    text = "Active";
  } else if (mins < 60) {
    tone = "recent";
    text = `${mins}m ago`;
  } else if (mins < 60 * 24) {
    tone = "recent";
    text = `${Math.floor(mins / 60)}h ago`;
  } else {
    tone = "stale";
    text = `${Math.floor(mins / (60 * 24))}d ago`;
  }

  return (
    <span className={`freshness freshness--${tone}`}>
      <span className="freshness__dot" aria-hidden="true" />
      {text}
    </span>
  );
}
