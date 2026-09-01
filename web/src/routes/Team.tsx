import { useState, type FormEvent } from "react";
import { useSession } from "../hooks/useSession";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { fmtDateTime, fmtStr } from "../api/format";
import { apiErrorMessage } from "../api/client";
import * as api from "../api/client";
import type { Role } from "../api/types";

const ROLE_RANK: Record<Role, number> = { owner: 4, admin: 3, member: 2, viewer: 1 };
const ALL_ROLES: Role[] = ["owner", "admin", "member", "viewer"];

function slugify(s: string): string {
  return s
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 63);
}

export function Team() {
  const { session } = useSession();
  if (!session) return null;

  const active = session.orgs.find((o) => o.slug === session.active_org);
  const teamOrgs = session.orgs.filter((o) => o.kind === "team");

  return (
    <div className="page">
      <div className="page__header">
        <h1>Team</h1>
        <p className="page__subtitle">Members, invites, and creating new teams</p>
      </div>

      {active?.kind === "team" ? (
        <TeamAdminOrMember slug={active.slug} name={active.name} role={active.role} />
      ) : (
        <section className="panel">
          <h2 className="panel__title">No team selected</h2>
          <p className="page__subtitle" style={{ marginBottom: 16 }}>
            {teamOrgs.length > 0
              ? "Switch to a team org from the header to manage its members and invites."
              : "You're only in your personal workspace. Create a team to invite others and see aggregated usage."}
          </p>
        </section>
      )}

      <CreateTeamPanel />
    </div>
  );
}

function TeamAdminOrMember({ slug, name, role }: { slug: string; name: string; role: Role }) {
  if (ROLE_RANK[role] < ROLE_RANK.admin) {
    return (
      <section className="panel">
        <h2 className="panel__title">{name}</h2>
        <p className="page__subtitle">
          You're a <span className="badge badge--neutral">{role}</span> of this team. Ask an admin to manage
          members or invites.
        </p>
      </section>
    );
  }
  return (
    <>
      <MembersPanel slug={slug} name={name} />
      <InvitesPanel slug={slug} callerRole={role} />
    </>
  );
}

function MembersPanel({ slug, name }: { slug: string; name: string }) {
  const members = useAsync(() => api.getMembers(slug), [slug]);
  return (
    <section className="panel">
      <h2 className="panel__title">Members of {name}</h2>
      <QueryBoundary state={members} isEmpty={(d) => d.members.length === 0} onRetry={members.reload}>
        {(data) => (
          <div className="table-scroll">
            <table className="data-table">
              <thead>
                <tr>
                  <th>Email</th>
                  <th>GitHub</th>
                  <th>Role</th>
                  <th>Joined</th>
                </tr>
              </thead>
              <tbody>
                {data.members.map((m) => (
                  <tr key={m.account_id}>
                    <td>{m.email}</td>
                    <td className="mono">{fmtStr(m.github_login)}</td>
                    <td>
                      <span className="badge badge--neutral">{m.role}</span>
                    </td>
                    <td>{fmtDateTime(m.created_at)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </QueryBoundary>
    </section>
  );
}

function InvitesPanel({ slug, callerRole }: { slug: string; callerRole: Role }) {
  const invites = useAsync(() => api.getInvites(slug), [slug]);
  const [role, setRole] = useState<Role>("member");
  const [expiresHours, setExpiresHours] = useState(24 * 7);
  const [maxUses, setMaxUses] = useState<string>("");
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  const [lastUrl, setLastUrl] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const grantableRoles = ALL_ROLES.filter((r) => ROLE_RANK[r] <= ROLE_RANK[callerRole]);

  async function onCreate(e: FormEvent) {
    e.preventDefault();
    setCreating(true);
    setCreateError(null);
    setCopied(false);
    try {
      const parsedMaxUses = maxUses.trim() === "" ? null : Number(maxUses);
      const result = await api.createInvite(slug, {
        role,
        expires_hours: expiresHours,
        max_uses: parsedMaxUses,
      });
      const absoluteUrl = `${window.location.origin}${result.url}`;
      setLastUrl(absoluteUrl);
      invites.reload();
    } catch (err) {
      setCreateError(apiErrorMessage(err));
    } finally {
      setCreating(false);
    }
  }

  async function copyUrl() {
    if (!lastUrl) return;
    try {
      await navigator.clipboard.writeText(lastUrl);
      setCopied(true);
    } catch {
      // Clipboard API unavailable -- the URL is still shown in the input
      // for manual copy.
    }
  }

  async function onRevoke(id: string) {
    try {
      await api.revokeInvite(slug, id);
      invites.reload();
    } catch {
      // Surfaced implicitly: the row simply won't show as revoked; a retry
      // via the button is available.
    }
  }

  return (
    <section className="panel">
      <h2 className="panel__title">Invites</h2>

      <form className="invite-form" onSubmit={onCreate}>
        <label className="field field--inline">
          <span className="field__label">Role</span>
          <select value={role} onChange={(e) => setRole(e.target.value as Role)}>
            {grantableRoles.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </select>
        </label>
        <label className="field field--inline">
          <span className="field__label">Expires (hours)</span>
          <input
            type="number"
            min={1}
            max={24 * 90}
            value={expiresHours}
            onChange={(e) => setExpiresHours(Number(e.target.value))}
          />
        </label>
        <label className="field field--inline">
          <span className="field__label">Max uses (optional)</span>
          <input
            type="number"
            min={1}
            placeholder="unlimited"
            value={maxUses}
            onChange={(e) => setMaxUses(e.target.value)}
          />
        </label>
        <button type="submit" className="btn btn--primary" disabled={creating}>
          {creating ? "Creating…" : "Create invite"}
        </button>
      </form>

      {createError && (
        <p className="login-card__error" role="alert">
          {createError}
        </p>
      )}

      {lastUrl && (
        <div className="invite-url-row">
          <input type="text" readOnly value={lastUrl} onFocus={(e) => e.currentTarget.select()} />
          <button type="button" className="btn btn--ghost" onClick={() => void copyUrl()}>
            {copied ? "Copied" : "Copy"}
          </button>
        </div>
      )}

      <QueryBoundary
        state={invites}
        isEmpty={(d) => d.invites.length === 0}
        emptyLabel="No invites yet"
        onRetry={invites.reload}
      >
        {(data) => (
          <div className="table-scroll">
            <table className="data-table">
              <thead>
                <tr>
                  <th>Role</th>
                  <th>Created</th>
                  <th>Expires</th>
                  <th>Uses</th>
                  <th>Status</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {data.invites.map((inv) => {
                  const expired = new Date(inv.expires_at).getTime() < Date.now();
                  const exhausted = inv.max_uses !== null && inv.uses >= inv.max_uses;
                  const inactive = inv.revoked || expired || exhausted;
                  return (
                    <tr key={inv.id} className={inactive ? "row-warn" : undefined}>
                      <td>
                        <span className="badge badge--neutral">{inv.role}</span>
                      </td>
                      <td>{fmtDateTime(inv.created_at)}</td>
                      <td>{fmtDateTime(inv.expires_at)}</td>
                      <td>
                        {inv.uses}
                        {inv.max_uses !== null ? ` / ${inv.max_uses}` : ""}
                      </td>
                      <td>
                        {inv.revoked ? "Revoked" : expired ? "Expired" : exhausted ? "Used up" : "Active"}
                      </td>
                      <td>
                        {!inv.revoked && (
                          <button type="button" className="btn btn--ghost btn--small" onClick={() => void onRevoke(inv.id)}>
                            Revoke
                          </button>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </QueryBoundary>
    </section>
  );
}

function CreateTeamPanel() {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugTouched, setSlugTouched] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function onNameChange(v: string) {
    setName(v);
    if (!slugTouched) setSlug(slugify(v));
  }

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      const org = await api.createOrg(name.trim(), slug.trim());
      await api.setActiveOrg(org.slug);
      window.location.assign("/team");
    } catch (err) {
      setError(apiErrorMessage(err));
      setSubmitting(false);
    }
  }

  if (!open) {
    return (
      <button type="button" className="btn btn--ghost" onClick={() => setOpen(true)}>
        + New team
      </button>
    );
  }

  return (
    <section className="panel">
      <h2 className="panel__title">Create a team</h2>
      <p className="page__subtitle" style={{ marginBottom: 16 }}>
        Bring your usage patterns to a team: aggregate MCP health, tool rankings, and cost across everyone
        who joins.
      </p>
      <form className="invite-form" onSubmit={onSubmit}>
        <label className="field field--inline">
          <span className="field__label">Team name</span>
          <input
            type="text"
            required
            value={name}
            onChange={(e) => onNameChange(e.target.value)}
            placeholder="Acme Inc"
          />
        </label>
        <label className="field field--inline">
          <span className="field__label">Slug</span>
          <input
            type="text"
            required
            value={slug}
            onChange={(e) => {
              setSlugTouched(true);
              setSlug(slugify(e.target.value));
            }}
            placeholder="acme"
          />
        </label>
        <button type="submit" className="btn btn--primary" disabled={submitting}>
          {submitting ? "Creating…" : "Create team"}
        </button>
        <button type="button" className="btn btn--ghost" onClick={() => setOpen(false)}>
          Cancel
        </button>
      </form>
      {error && (
        <p className="login-card__error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
