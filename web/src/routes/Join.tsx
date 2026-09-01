import { useEffect, useState } from "react";
import { useRouter, Link } from "../router/Router";
import { useSession, stashPendingInvite } from "../hooks/useSession";
import * as api from "../api/client";
import { ApiError, apiErrorMessage } from "../api/client";
import type { InviteInfo, WebConfig } from "../api/types";

type State =
  | { status: "loading" }
  | { status: "not-found" }
  | { status: "ready"; info: InviteInfo }
  | { status: "joining"; info: InviteInfo }
  | { status: "joined"; orgSlug: string }
  | { status: "error"; message: string };

/** `/join/:token` — the account-model contract's "GET /join/<token> (auth
 * required) → join page", as an SPA route: the server just hands back the
 * app shell for this path (see crates/cloud/src/lib.rs), and this component
 * does the rest by calling GET /web/invites/:token + POST /join/:token. */
export function Join({ token }: { token: string }) {
  const { status: sessionStatus } = useSession();

  if (sessionStatus === "loading") {
    return (
      <div className="login-page">
        <div className="state-panel state-panel--loading" role="status">
          <span className="spinner" aria-hidden="true" />
          Loading…
        </div>
      </div>
    );
  }

  if (sessionStatus === "anon") {
    return <JoinSignInPrompt token={token} />;
  }

  return <JoinConfirm token={token} />;
}

function JoinSignInPrompt({ token }: { token: string }) {
  const [config, setConfig] = useState<WebConfig | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .getConfig()
      .then((c) => {
        if (!cancelled) setConfig(c);
      })
      .catch(() => {
        if (!cancelled) setConfig({ github_oauth: false, legacy_login: true });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="login-page">
      <div className="login-card">
        <div className="login-card__brand">
          <span className="brand-mark" aria-hidden="true">
            K
          </span>
          <span className="brand-name">kikimimi</span>
        </div>
        <p className="login-card__subtitle">Sign in to view and accept this invite.</p>
        {config?.github_oauth && (
          <a href="/auth/github" className="btn btn--primary btn--github" onClick={() => stashPendingInvite(token)}>
            Sign in with GitHub
          </a>
        )}
        {(config?.legacy_login ?? true) && (
          <Link to="/login" className="btn btn--ghost" onClick={() => stashPendingInvite(token)}>
            Log in with email
          </Link>
        )}
      </div>
    </div>
  );
}

function unusableReason(info: InviteInfo): string {
  if (info.revoked) return "This invite has been revoked by an admin.";
  if (info.expired) return "This invite has expired.";
  if (info.exhausted) return "This invite has already been used up.";
  return "This invite is no longer valid.";
}

function JoinConfirm({ token }: { token: string }) {
  const { navigate } = useRouter();
  const [state, setState] = useState<State>({ status: "loading" });

  useEffect(() => {
    let cancelled = false;
    api
      .getInviteInfo(token)
      .then((info) => {
        if (!cancelled) setState({ status: "ready", info });
      })
      .catch((err) => {
        if (cancelled) return;
        if (err instanceof ApiError && err.status === 404) {
          setState({ status: "not-found" });
        } else {
          setState({ status: "error", message: apiErrorMessage(err) });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [token]);

  async function acceptInvite(info: InviteInfo) {
    setState({ status: "joining", info });
    try {
      const result = await api.joinInvite(token);
      // Switch the browser straight into the org just joined, then do a
      // full reload -- every /web/q/* view reads its org from the session
      // cookie server-side, so this is the simplest way to make sure
      // nothing on the next screen is stale.
      await api.setActiveOrg(result.org_slug);
      setState({ status: "joined", orgSlug: result.org_slug });
      window.location.assign("/");
    } catch (err) {
      setState({
        status: "error",
        message: apiErrorMessage(err),
      });
    }
  }

  return (
    <div className="login-page">
      <div className="login-card">
        <div className="login-card__brand">
          <span className="brand-mark" aria-hidden="true">
            K
          </span>
          <span className="brand-name">kikimimi</span>
        </div>

        {state.status === "loading" && (
          <div className="state-panel state-panel--loading" role="status">
            <span className="spinner" aria-hidden="true" />
            Loading invite…
          </div>
        )}

        {state.status === "not-found" && (
          <>
            <p className="login-card__error" role="alert">
              This invite link is invalid.
            </p>
            <button type="button" className="btn btn--ghost" onClick={() => navigate("/")}>
              Go to kikimimi
            </button>
          </>
        )}

        {(state.status === "ready" || state.status === "joining") && !state.info.usable && (
          <>
            <p className="login-card__error" role="alert">
              {unusableReason(state.info)}
            </p>
            <button type="button" className="btn btn--ghost" onClick={() => navigate("/")}>
              Go to kikimimi
            </button>
          </>
        )}

        {(state.status === "ready" || state.status === "joining") && state.info.usable && (
          <>
            <p className="login-card__subtitle">
              You've been invited to join <strong>{state.info.org_name}</strong> as{" "}
              <span className="badge badge--neutral">{state.info.role}</span>.
            </p>
            <button
              type="button"
              className="btn btn--primary"
              disabled={state.status === "joining"}
              onClick={() => void acceptInvite(state.info)}
            >
              {state.status === "joining" ? "Joining…" : `Join ${state.info.org_name}`}
            </button>
          </>
        )}

        {state.status === "joined" && (
          <p className="login-card__subtitle">You're in! Taking you to {state.orgSlug}…</p>
        )}

        {state.status === "error" && (
          <>
            <p className="login-card__error" role="alert">
              Failed to load invite: {state.message}
            </p>
            <button type="button" className="btn btn--ghost" onClick={() => navigate("/")}>
              Go to kikimimi
            </button>
          </>
        )}
      </div>
    </div>
  );
}
