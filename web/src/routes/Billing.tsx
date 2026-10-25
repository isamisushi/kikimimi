import { useEffect, useState, type FormEvent } from "react";
import { useSession } from "../hooks/useSession";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { StatTile } from "../components/StatTile";
import * as api from "../api/client";
import { apiErrorMessage } from "../api/client";
import type { BillingInfo, Invoice, Role } from "../api/types";

const ROLE_RANK: Record<Role, number> = { owner: 4, admin: 3, member: 2, viewer: 1 };

const yenFormat = new Intl.NumberFormat("ja-JP", { style: "currency", currency: "JPY" });
function yen(amount: number): string {
  return yenFormat.format(amount);
}

function fmtDay(value: string | null): string {
  if (!value) return "—";
  const d = new Date(value);
  return Number.isNaN(d.getTime()) ? value : d.toLocaleDateString();
}

/** `/billing` — a team's seats, billing details and invoices (crates/cloud
 * billing.rs). Up to `free_seats` seats are free; past that the team is
 * billed monthly by invoice and pays by bank transfer. Admin and owner
 * only, like the server. */
export function Billing() {
  const { session } = useSession();
  if (!session) return null;
  const active = session.orgs.find((o) => o.slug === session.active_org);

  return (
    <div className="page">
      <div className="page__header">
        <h1>Billing</h1>
        <p className="page__subtitle">Seats, billing details, and invoices for this team</p>
      </div>
      {!active || active.kind !== "team" ? (
        <section className="panel">
          <p className="page__subtitle">
            Personal workspaces are free. Switch to a team from the header to see its billing.
          </p>
        </section>
      ) : ROLE_RANK[active.role] < ROLE_RANK.admin ? (
        <section className="panel">
          <p className="page__subtitle">
            You're a <span className="badge badge--neutral">{active.role}</span> of this team. Ask an admin to
            manage billing.
          </p>
        </section>
      ) : (
        <BillingPanels slug={active.slug} />
      )}
    </div>
  );
}

function BillingPanels({ slug }: { slug: string }) {
  const billing = useAsync<BillingInfo>(() => api.getBilling(slug), [slug]);
  return (
    <QueryBoundary state={billing} onRetry={billing.reload}>
      {(info) => (
        <>
          {info.read_only && (
            <div className="callout callout--warn" role="alert">
              This team is read-only because an invoice is overdue. Each machine keeps its data and sends it
              once the invoice is paid. If you already paid, reply to the reminder mail.
            </div>
          )}
          <SeatsPanel info={info} />
          <ProfilePanel slug={slug} info={info} onSaved={billing.reload} />
          {info.profile && <InvoicesPanel slug={slug} />}
        </>
      )}
    </QueryBoundary>
  );
}

function SeatsPanel({ info }: { info: BillingInfo }) {
  const { base_monthly_jpy: base, seat_monthly_jpy: seat, tax_rate_percent: tax } = info.prices;
  const trialEnds = info.profile ? new Date(info.profile.trial_ends_at) : null;
  const inTrial = trialEnds !== null && trialEnds.getTime() > Date.now();
  return (
    <section className="panel">
      <h2 className="panel__title">Seats</h2>
      <div className="stat-grid">
        <StatTile label="Seats in use" value={String(info.seats)} />
        <StatTile label="Free seats" value={String(info.free_seats)} />
      </div>
      <p className="panel__note" style={{ marginTop: 12 }}>
        Owners, admins and members take a seat; viewers are free. Up to {info.free_seats} seats cost nothing.
        From the {info.free_seats + 1}th seat on, every seat is billed, prorated by day.
        {base !== null && seat !== null && (
          <>
            {" "}
            The price is {yen(base)} a month plus {yen(seat)} a seat a month, before {tax}% tax.
          </>
        )}{" "}
        Invoices go out monthly by mail, paid by bank transfer.
      </p>
      {info.billing_required_for_next_seat && (
        <div className="callout callout--info">
          All free seats are in use. Add billing details below before the next member can join.
        </div>
      )}
      {inTrial && trialEnds && (
        <div className="callout callout--info">Free trial until {trialEnds.toLocaleDateString()}.</div>
      )}
    </section>
  );
}

function ProfilePanel({ slug, info, onSaved }: { slug: string; info: BillingInfo; onSaved: () => void }) {
  const p = info.profile;
  const [company, setCompany] = useState(p?.company_name ?? "");
  const [contact, setContact] = useState(p?.contact_name ?? "");
  const [email, setEmail] = useState(p?.billing_email ?? "");
  const [address, setAddress] = useState(p?.address ?? "");
  const [accept, setAccept] = useState(p !== null);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    setCompany(p?.company_name ?? "");
    setContact(p?.contact_name ?? "");
    setEmail(p?.billing_email ?? "");
    setAddress(p?.address ?? "");
  }, [p]);

  async function onSubmit(e: FormEvent) {
    e.preventDefault();
    setSaving(true);
    setMessage(null);
    try {
      await api.putBilling(slug, {
        company_name: company,
        contact_name: contact,
        billing_email: email,
        address,
        accept_terms: accept,
      });
      setMessage({ ok: true, text: "Saved." });
      onSaved();
    } catch (err) {
      setMessage({ ok: false, text: apiErrorMessage(err) });
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="panel">
      <h2 className="panel__title">Billing details</h2>
      <p className="panel__note">Invoices are addressed and mailed to these details.</p>
      <form className="billing-form" onSubmit={onSubmit}>
        <label className="field">
          <span className="field__label">Company name</span>
          <input value={company} onChange={(e) => setCompany(e.target.value)} required autoComplete="organization" />
        </label>
        <label className="field">
          <span className="field__label">Contact person</span>
          <input value={contact} onChange={(e) => setContact(e.target.value)} required autoComplete="name" />
        </label>
        <label className="field">
          <span className="field__label">Invoice email</span>
          <input
            type="email"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            required
            autoComplete="email"
            placeholder="billing@company.com"
          />
        </label>
        <label className="field">
          <span className="field__label">Address</span>
          <textarea value={address} onChange={(e) => setAddress(e.target.value)} required rows={3} />
        </label>
        {p === null && (
          <label className="field field--checkbox">
            <input type="checkbox" checked={accept} onChange={(e) => setAccept(e.target.checked)} required />
            <span>I accept the kikimimi terms for paid teams on behalf of this company.</span>
          </label>
        )}
        {message && (
          <p className={message.ok ? "panel__note" : "login-card__error"} role={message.ok ? "status" : "alert"}>
            {message.text}
          </p>
        )}
        <button type="submit" className="btn btn--primary" disabled={saving}>
          {saving ? "Saving…" : p === null ? "Add billing details" : "Save changes"}
        </button>
      </form>
    </section>
  );
}

const STATUS_LABEL: Record<Invoice["status"], string> = {
  issued: "Unpaid",
  paid: "Paid",
  void: "Cancelled",
};

function InvoicesPanel({ slug }: { slug: string }) {
  const invoices = useAsync(() => api.getInvoices(slug), [slug]);
  return (
    <section className="panel">
      <h2 className="panel__title">Invoices</h2>
      <QueryBoundary
        state={invoices}
        isEmpty={(d) => d.invoices.length === 0}
        emptyLabel="No invoices yet"
        onRetry={invoices.reload}
      >
        {(data) => (
          <div className="table-scroll">
            <table className="data-table">
              <thead>
                <tr>
                  <th>Number</th>
                  <th>Month</th>
                  <th className="align-right">Total (tax incl.)</th>
                  <th>Status</th>
                  <th>Due</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {data.invoices.map((inv) => (
                  <tr key={inv.id}>
                    <td className="mono">
                      {inv.number}
                      {inv.kind === "credit" && (
                        <span className="badge badge--neutral" title={`Cancels ${inv.credits_number ?? ""}`}>
                          credit note
                        </span>
                      )}
                    </td>
                    <td>{inv.period}</td>
                    <td className="align-right">{yen(inv.total_jpy)}</td>
                    <td>{inv.kind === "credit" ? "—" : STATUS_LABEL[inv.status]}</td>
                    <td>{inv.kind === "credit" ? "—" : fmtDay(inv.due_date)}</td>
                    <td>
                      <a href={api.invoicePageUrl(slug, inv.id)} target="_blank" rel="noreferrer">
                        Open
                      </a>
                    </td>
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
