import brandMark from "../assets/kikimimi-mark.svg";
import { useEffect, useState, type FormEvent } from "react";
import { useSession } from "../hooks/useSession";
import { ApiError, apiErrorMessage, getConfig, requestEmailLink } from "../api/client";
import type { WebConfig } from "../api/types";

/** GitHub's mark, inlined so the button doesn't depend on an external
 * icon font/CDN. */
function GithubMark() {
  return (
    <svg width="18" height="18" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8Z" />
    </svg>
  );
}

/** Google's "G", inlined for the same reason as GithubMark. */
function GoogleMark() {
  return (
    <svg width="18" height="18" viewBox="0 0 48 48" aria-hidden="true">
      <path fill="#EA4335" d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z" />
      <path fill="#4285F4" d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z" />
      <path fill="#FBBC05" d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z" />
      <path fill="#34A853" d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z" />
    </svg>
  );
}

/** One-time sign-in link by mail (`POST /web/login/email`). Also how a new
 * person without GitHub or Google signs up. */
function EmailLinkForm() {
  const [email, setEmail] = useState("");
  const [sentTo, setSentTo] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    setError(null);
    setSending(true);
    try {
      await requestEmailLink(email.trim());
      setSentTo(email.trim());
    } catch (err) {
      if (err instanceof ApiError && err.status === 429) {
        setError("Too many sign-in mails for this address. Wait a few minutes and try again.");
      } else if (err instanceof ApiError && err.status === 0) {
        setError("Could not connect to the kikimimi server.");
      } else {
        setError(apiErrorMessage(err));
      }
    } finally {
      setSending(false);
    }
  }

  if (sentTo) {
    return (
      <div className="callout callout--info" role="status">
        We sent a sign-in link to <strong>{sentTo}</strong>. Open it on this device within 15 minutes.{" "}
        <button type="button" className="btn btn--ghost btn--small" onClick={() => setSentTo(null)}>
          Use another address
        </button>
      </div>
    );
  }

  return (
    <form onSubmit={onSubmit}>
      <label className="field">
        <span className="field__label">Work email</span>
        <input
          type="email"
          name="email"
          autoComplete="email"
          required
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          placeholder="you@company.com"
        />
      </label>
      {error && (
        <p className="login-card__error" role="alert">
          {error}
        </p>
      )}
      <button type="submit" className="btn btn--primary" disabled={sending}>
        {sending ? "Sending…" : "Email me a sign-in link"}
      </button>
    </form>
  );
}

export function Login() {
  const { login } = useSession();
  const [email, setEmail] = useState("");
  const [inviteCode, setInviteCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [config, setConfig] = useState<WebConfig | null>(null);
  const [configError, setConfigError] = useState(false);

  useEffect(() => {
    let cancelled = false;
    getConfig()
      .then((c) => {
        if (!cancelled) setConfig(c);
      })
      .catch(() => {
        // Can't reach the server at all -- fall back to showing the legacy
        // form (the more universally-applicable option) rather than a
        // blank page.
        if (!cancelled) setConfigError(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      await login(email.trim(), inviteCode.trim());
    } catch (err) {
      if (err instanceof ApiError && err.status === 403) {
        setError("Incorrect email or invite code.");
      } else if (err instanceof ApiError) {
        setError(`Login failed (${err.status})`);
      } else {
        setError("Could not connect to the kikimimi server.");
      }
    } finally {
      setSubmitting(false);
    }
  }

  const showGithub = config?.github_oauth === true;
  const showGoogle = config?.google_oauth === true;
  const showEmailLink = config?.email_login === true;
  const showProviders = showGithub || showGoogle;
  // While config is still loading, don't flash the legacy form if this
  // deployment turns out to be GitHub-only -- but do show it once we know
  // for sure it's wanted, or if we couldn't even reach the server.
  const showLegacy = config ? config.legacy_login : configError;
  const loadingConfig = !config && !configError;

  return (
    <div className="login-page">
      <div className="login-card">
        <div className="login-card__brand">
          <img className="brand-mark" src={brandMark} alt="" width="32" height="32" />
          <span className="brand-name">kikimimi</span>
        </div>
        <p className="login-card__subtitle">
          Visibility into your team's AI agent usage
        </p>

        {loadingConfig && (
          <div className="state-panel state-panel--loading" role="status">
            <span className="spinner" aria-hidden="true" />
            Loading…
          </div>
        )}

        {showGoogle && (
          <a href="/auth/google" className="btn btn--google">
            <GoogleMark />
            Sign in with Google
          </a>
        )}

        {showGithub && (
          <a href="/auth/github" className="btn btn--primary btn--github">
            <GithubMark />
            Sign in with GitHub
          </a>
        )}

        {showProviders && (showEmailLink || showLegacy) && (
          <div className="login-card__divider" role="separator">
            <span>or</span>
          </div>
        )}

        {showEmailLink && <EmailLinkForm />}

        {showEmailLink && showLegacy && (
          <div className="login-card__divider" role="separator">
            <span>or use an invite code</span>
          </div>
        )}

        {showLegacy && (
          <form onSubmit={onSubmit}>
            <label className="field">
              <span className="field__label">Email address</span>
              <input
                type="email"
                name="email"
                autoComplete="email"
                required
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="you@company.com"
              />
            </label>

            <label className="field">
              <span className="field__label">Invite code</span>
              <input
                type="text"
                name="invite_code"
                autoComplete="off"
                required
                value={inviteCode}
                onChange={(e) => setInviteCode(e.target.value)}
                placeholder="KIKIMIMI-XXXXXX"
              />
            </label>

            {error && (
              <p className="login-card__error" role="alert">
                {error}
              </p>
            )}

            <button type="submit" className="btn btn--primary" disabled={submitting}>
              {submitting ? "Logging in…" : "Log in"}
            </button>
          </form>
        )}

        {!loadingConfig && !showProviders && !showEmailLink && !showLegacy && (
          <p className="login-card__error" role="alert">
            No login method is available on this server. Contact your administrator.
          </p>
        )}
      </div>
    </div>
  );
}
