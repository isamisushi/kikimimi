import { useState } from "react";
import { apiErrorMessage, deleteInventoryPerson, getInventory, saveInventory } from "../api/client";
import { contractTotals, importPeople, type InventoryPerson, type InventoryRow } from "../api/inventory";
import { useAsync } from "../hooks/useAsync";
import { useSession } from "../hooks/useSession";
import { StatTile } from "../components/StatTile";

const TOOLS = ["claude-code", "codex", "cursor", "opencode", "copilot", "other"];
const blank = (): InventoryPerson => ({ email: "", name: "", department: "", account_id: null, assignments: [] });
const money = (n: number, currency: string) => new Intl.NumberFormat(undefined, { style: "currency", currency }).format(n);
const template = "email,name,department,tool,plan,allocated_model,monthly_amount,currency,account_id\nalex@example.com,Alex,Engineering,claude-code,Team,,30,USD,\nsam@example.com,Sam,Operations,,,,,,\n";

export function Inventory() {
  const { session } = useSession();
  const org = session?.orgs.find(o => o.slug === session.active_org);
  if (session?.local) return <div className="page"><h1>AI allocation</h1><p>Open a cloud workspace to manage your roster and contracts.</p></div>;
  if (!org || !["admin", "owner"].includes(org.role)) return <div className="page"><h1>AI allocation</h1><p>Only workspace admins and owners can view or edit this page.</p></div>;
  return <InventoryPage key={org.slug} />;
}
function InventoryPage() {
  const state = useAsync(getInventory, []);
  const [draft, setDraft] = useState<InventoryPerson | null>(null);
  const [editing, setEditing] = useState(false);
  const [csv, setCsv] = useState<InventoryPerson[] | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [department, setDepartment] = useState("");
  const [removeId, setRemoveId] = useState<string | null>(null);
  async function save(people: InventoryPerson[]) {
    setBusy(true); setError(""); setNotice("");
    try { const result = await saveInventory(people); setNotice(`Saved ${result.saved} ${result.saved === 1 ? "person" : "people"}.`); setDraft(null); setCsv(null); state.reload(); }
    catch (e) { setError(apiErrorMessage(e)); }
    finally { setBusy(false); }
  }
  async function remove(id: string) {
    setBusy(true); setError("");
    try { await deleteInventoryPerson(id); setRemoveId(null); state.reload(); }
    catch (e) { setError(apiErrorMessage(e)); }
    finally { setBusy(false); }
  }
  const data = state.status === "ok" ? state.data : null;
  const people = data?.people.filter(p => !department || p.department === department) ?? [];
  const assigned = people.filter(p => p.assignments.length > 0);
  // Count each person once, regardless of assignments or number of devices.
  const observed = assigned.filter(p => p.assignments.some(a => p.usage.some(u => u.agent === a.tool)));
  const totals = contractTotals(people);
  const unknownCosts = people.flatMap(p => p.assignments).filter(a => a.monthly_amount === null).length;
  return <div className="page">
    <div className="page__header"><h1>AI allocation</h1><p className="page__subtitle">People, assigned tools, monthly contracts and observed usage — including people with no activity records.</p></div>
    {error && <div role="alert" className="callout callout--warn">{error}</div>}
    {notice && <p role="status">{notice}</p>}
    {state.status === "loading" && <p role="status">Loading roster…</p>}
    {state.status === "error" && <div role="alert">{state.error} <button className="btn" onClick={state.reload}>Retry</button></div>}
    {data && <>
      <div className="inventory-toolbar">
        <label className="field field--inline"><span className="field__label">Department</span><select value={department} onChange={e => setDepartment(e.target.value)}><option value="">All departments</option>{[...new Set(data.people.map(p => p.department))].filter(Boolean).sort().map(d => <option key={d}>{d}</option>)}</select></label>
        <button className="btn btn--primary" disabled={busy} onClick={() => { setDraft(blank()); setEditing(false); setCsv(null); }}>Add person</button>
        <label className="field field--inline"><span className="field__label">Import CSV</span><input type="file" accept=".csv,text/csv" disabled={busy} onChange={async e => {
          const file = e.target.files?.[0]; e.target.value = ""; if (!file) return;
          setCsv(null); setError("");
          try { if (file.size > 2_000_000) throw new Error("CSV must be smaller than 2 MB"); setCsv(importPeople(await file.text())); setDraft(null); }
          catch (err) { setError(apiErrorMessage(err)); }
        }} /></label>
        <a className="btn btn--ghost" download="ai-roster-template.csv" href={`data:text/csv;charset=utf-8,${encodeURIComponent(template)}`}>CSV template</a>
      </div>
      <div className="stat-grid">
        <StatTile label="People in roster" value={String(people.length)} hint={`${assigned.length} assigned at least one tool`} />
        <StatTile label="Observed use / assigned" value={assigned.length ? `${observed.length} / ${assigned.length} (${Math.round(observed.length / assigned.length * 100)}%)` : "–"} hint="Last 30 UTC days · assigned tools only" />
        <StatTile label="Monthly contracts" value={totals.length ? totals.map(([c,n]) => money(n,c)).join(" + ") : "–"} hint={`${unknownCosts} assignments with unknown amounts · currencies kept separate`} />
      </div>
      <p className="page__subtitle">Recorded use is a lower bound, not an adoption score. Missing records can mean no use, disconnected collection or unshared activity. Automatic collection currently covers Claude Code and Codex. Contract amounts are manually entered monthly equivalents, not invoices; observed API estimates below are separate and may be incomplete.</p>
      {csv && <section className="panel"><h2>Review import</h2><p>{csv.length} people, {csv.reduce((n,p) => n + p.assignments.length,0)} assignments. Matching emails will have their details and all assignments replaced. Other people are kept.</p><div className="inventory-import-preview"><table><thead><tr><th>Person</th><th>Department</th><th>Assignments / monthly contracts</th><th>Action</th></tr></thead><tbody>{csv.map(p => <tr key={p.email}><td>{p.name}<br />{p.email}</td><td>{p.department || "–"}</td><td>{p.assignments.length ? p.assignments.map(a => <div key={a.tool}>{a.tool} · {a.plan || "Unspecified plan"} · {a.allocated_model || "Unspecified model"} · {a.monthly_amount === null ? "Unknown amount" : money(a.monthly_amount,a.currency)}</div>) : "Not assigned"}</td><td>{data.people.some(existing => existing.email === p.email) ? "Replace" : "Add"}</td></tr>)}</tbody></table></div><button className="btn btn--primary" disabled={busy} onClick={() => void save(csv)}>Save import</button> <button className="btn btn--ghost" disabled={busy} onClick={() => setCsv(null)}>Cancel</button></section>}
      {draft && <section className="panel"><h2>{editing ? "Edit person" : "Add person"}</h2>
        <form onSubmit={e => { e.preventDefault(); void save([draft]); }}>
          <fieldset disabled={busy} className="inventory-form">
            {(["email", "name", "department"] as const).map(key => <label className="field" key={key}><span className="field__label">{key}</span><input value={draft[key]} required={key !== "department"} type={key === "email" ? "email" : "text"} readOnly={key === "email" && editing} maxLength={key === "email" ? 254 : 200} onChange={e => setDraft({ ...draft, [key]: e.target.value })} /></label>)}
            <label className="field"><span className="field__label">Usage account</span><select value={draft.account_id ?? ""} onChange={e => setDraft({ ...draft, account_id: e.target.value || null })}><option value="">Match workspace member by email</option>{data.members.map(m => <option key={m.id} value={m.id}>{m.email}</option>)}</select></label>
            <p>Link a workspace member explicitly when their sign-in email differs from the roster email. A roster entry does not invite or provision an account.</p>
            {draft.assignments.map((a,i) => <div className="inventory-assignment" key={i}>
              <label className="field"><span className="field__label">Tool</span><select value={a.tool} onChange={e => setDraft({ ...draft, assignments: draft.assignments.map((v,j) => j === i ? { ...v, tool: e.target.value } : v) })}>{TOOLS.map(t => <option key={t}>{t}</option>)}</select></label>
              {(["plan", "allocated_model"] as const).map(key => <label className="field" key={key}><span className="field__label">{key === "plan" ? "Plan" : "Allocated model(s)"}</span><input maxLength={200} value={a[key]} onChange={e => setDraft({ ...draft, assignments: draft.assignments.map((v,j) => j === i ? { ...v, [key]: e.target.value } : v) })} /></label>)}
              <label className="field"><span className="field__label">Monthly contract amount</span><input type="number" min="0" max="1000000000" step="0.01" placeholder="Unknown" value={a.monthly_amount ?? ""} onChange={e => setDraft({ ...draft, assignments: draft.assignments.map((v,j) => j === i ? { ...v, monthly_amount: e.target.value === "" ? null : Number(e.target.value) } : v) })} /></label>
              <label className="field"><span className="field__label">Currency</span><select value={a.currency} onChange={e => setDraft({ ...draft, assignments: draft.assignments.map((v,j) => j === i ? { ...v, currency: e.target.value } : v) })}>{["USD","JPY","EUR","GBP"].map(c => <option key={c}>{c}</option>)}</select></label>
              <button type="button" className="btn btn--ghost" onClick={() => setDraft({ ...draft, assignments: draft.assignments.filter((_,j) => j !== i) })}>Remove assignment</button>
            </div>)}
            <div><button type="button" className="btn btn--ghost" disabled={draft.assignments.length >= TOOLS.length} onClick={() => setDraft({ ...draft, assignments: [...draft.assignments, { tool: TOOLS.find(t => !draft.assignments.some(a => a.tool === t)) ?? "other", plan:"", allocated_model:"", monthly_amount:null, currency:"USD" }] })}>Add tool</button></div>
            <div><button className="btn btn--primary" type="submit">Save person</button> <button className="btn btn--ghost" type="button" onClick={() => setDraft(null)}>Cancel</button></div>
          </fieldset>
        </form>
      </section>}
      <section className="panel inventory-table"><table><caption>Roster and observed usage · {new Date(data.from).toISOString().slice(0,10)}–{new Date(data.to).toISOString().slice(0,10)} UTC</caption><thead><tr>{["Person / department", "Assigned tool / model", "Monthly contract", "Observed use", "Observed models / API estimate", ""].map((h,i) => <th key={i}>{h}</th>)}</tr></thead><tbody>
        {people.map(p => <tr key={p.id}><td><strong>{p.name}</strong><div>{p.email}</div><div className="text-muted">{p.department || "No department"}</div></td><td>{p.assignments.length ? p.assignments.map(a => <div className="inventory-item" key={a.tool}><strong>{a.tool}</strong> · {a.plan || "Plan unspecified"}<div>{a.allocated_model || "Model not specified"}</div></div>) : "Not assigned"}</td><td>{p.assignments.map(a => <div className="inventory-item" key={a.tool}>{a.tool}: {a.monthly_amount === null ? "Unknown" : money(a.monthly_amount,a.currency)}</div>)}</td><td><UsageStatus person={p} /></td><td>{p.usage.map(u => <div className="inventory-item" key={u.agent}><strong>{u.agent}</strong><div>{u.models?.join(", ") || "Model unknown"}</div><div>{u.estimated_cost_usd === null ? "Cost unknown" : `${money(u.estimated_cost_usd,"USD")} estimated`}</div><small>{u.priced_requests}/{u.total_requests} requests priced</small></div>)}</td><td>
          <button className="btn btn--ghost" disabled={busy} onClick={() => { const { email,name,department,account_id,assignments } = p; setDraft({email,name,department,account_id,assignments:assignments.map(a=>({...a}))}); setEditing(true); setCsv(null); }}>Edit</button>
          {removeId === p.id ? <><span>Remove roster entry?</span><button className="btn" disabled={busy} onClick={() => void remove(p.id)}>Confirm removal</button><button className="btn btn--ghost" disabled={busy} onClick={() => setRemoveId(null)}>Cancel</button></> : <button className="btn btn--ghost" disabled={busy} onClick={() => setRemoveId(p.id)}>Remove</button>}
        </td></tr>)}
      </tbody></table>{!people.length && <p>Add people or import a CSV to include everyone you plan to support.</p>}</section>
    </>}
  </div>;
}
function UsageStatus({ person: p }: { person: InventoryRow }) {
  return <>{!p.matched_account_id && <div>{p.identity_conflict ? "Conflicting account links — edit roster" : "No linked workspace member"}</div>}{p.assignments.map(a => {
    const u = p.usage.find(u => u.agent === a.tool);
    return <div className="inventory-item" key={a.tool}><strong>{a.tool}</strong><div>{u ? `${u.active_days} active days · ${u.sessions} sessions` : !["claude-code","codex"].includes(a.tool) ? "Collection not supported" : "No activity observed · usage unknown"}</div>{u && <small>Last observed {new Date(u.last_seen_at).toLocaleString()}</small>}</div>;
  })}{p.usage.filter(u => !p.assignments.some(a => a.tool === u.agent)).map(u => <div key={u.agent}>{u.agent}: observed, not assigned in roster</div>)}</>;
}
