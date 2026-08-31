import { useState, type FormEvent } from "react";
import { useSession } from "../hooks/useSession";
import { ApiError } from "../api/client";

export function Login() {
  const { login } = useSession();
  const [email, setEmail] = useState("");
  const [inviteCode, setInviteCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

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
        setError("Could not connect to the guru server.");
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="login-page">
      <form className="login-card" onSubmit={onSubmit}>
        <div className="login-card__brand">
          <span className="brand-mark" aria-hidden="true">
            G
          </span>
          <span className="brand-name">guru</span>
        </div>
        <p className="login-card__subtitle">
          Visibility into your team's AI agent usage
        </p>

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
            placeholder="GURU-XXXXXX"
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
    </div>
  );
}
