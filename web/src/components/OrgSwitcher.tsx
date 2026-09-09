import { useState } from "react";
import { useSession } from "../hooks/useSession";
import * as api from "../api/client";

/** Header dropdown listing the account's orgs (`GET /web/me`'s `orgs`);
 * picking one calls `POST /web/active-org` then does a full reload so every
 * /web/q/* view on the current page re-reads its org from the (now updated)
 * session cookie -- simpler and more robust than threading an org-change
 * event through every data-fetching route. */
export function OrgSwitcher() {
  const { session } = useSession();
  const [switching, setSwitching] = useState(false);
  const [error, setError] = useState(false);

  if (!session) return null;

  async function onChange(slug: string) {
    if (!session || slug === session.active_org) return;
    setSwitching(true);
    setError(false);
    try {
      await api.setActiveOrg(slug);
      window.location.reload();
    } catch {
      setSwitching(false);
      setError(true);
    }
  }

  return (
    <label className="org-switcher">
      <span className="sr-only">Viewing workspace</span>
      <select
        value={session.active_org}
        disabled={switching}
        onChange={(e) => void onChange(e.target.value)}
      >
        {session.orgs.map((o) => (
          <option key={o.slug} value={o.slug}>
            {o.kind === "personal" ? "Personal" : o.name}
          </option>
        ))}
      </select>
      {error && <span role="alert">Could not switch workspace. Try again.</span>}
    </label>
  );
}
