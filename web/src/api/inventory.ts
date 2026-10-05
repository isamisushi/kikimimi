export interface Assignment {
  tool: string;
  plan: string;
  allocated_model: string;
  monthly_amount: number | null;
  currency: string;
}
export interface InventoryPerson {
  email: string;
  name: string;
  department: string;
  account_id: string | null;
  assignments: Assignment[];
}
export interface InventoryUsage {
  agent: string;
  sessions: number;
  active_days: number;
  last_seen_at: number;
  models: string[] | null;
  estimated_cost_usd: number | null;
  priced_requests: number;
  total_requests: number;
}
export interface InventoryRow extends InventoryPerson {
  id: string;
  matched_account_id: string | null;
  identity_conflict?: boolean;
  usage: InventoryUsage[];
}
export interface InventoryResponse {
  people: InventoryRow[];
  members: { id: string; email: string }[];
  from: number;
  to: number;
}

// RFC-style quoted fields, including commas/newlines and doubled quotes.
export function parseCsv(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [], field = "", quoted = false, closed = false;
  text = text.replace(/^\uFEFF/, "");
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') { field += '"'; i++; }
      else if (c === '"') { quoted = false; closed = true; }
      else field += c;
    } else if (c === ',' || c === '\n' || c === '\r') {
      row.push(field); field = ""; closed = false;
      if (c !== ',') {
        if (row.some(v => v.trim())) rows.push(row);
        row = [];
        if (c === '\r' && text[i + 1] === '\n') i++;
      }
    } else if (c === '"' && !field && !closed) quoted = true;
    else {
      if (closed || c === '"') throw new Error("Invalid CSV quoting");
      field += c;
    }
  }
  if (quoted) throw new Error("Unclosed CSV quote");
  row.push(field);
  if (row.some(v => v.trim())) rows.push(row);
  return rows;
}
export function importPeople(text: string): InventoryPerson[] {
  const [header, ...rows] = parseCsv(text);
  const columns = ["email", "name", "department", "tool", "plan", "allocated_model", "monthly_amount", "currency", "account_id"];
  if (!header || columns.some(c => !header.includes(c)) || new Set(header).size !== header.length || header.some(c => !columns.includes(c))) throw new Error(`CSV header must contain: ${columns.join(",")}`);
  const people = new Map<string, InventoryPerson>();
  for (const [index, row] of rows.entries()) {
    if (row.length !== header.length) throw new Error(`Row ${index + 2}: wrong number of columns`);
    const v = Object.fromEntries(header.map((h, i) => [h, row[i].trim()]));
    const email = v.email.toLowerCase();
    if (!email || !v.name) throw new Error(`Row ${index + 2}: email and name are required`);
    const p = people.get(email) ?? { email, name: v.name, department: v.department, account_id: v.account_id || null, assignments: [] };
    if (p.name !== v.name || p.department !== v.department || p.account_id !== (v.account_id || null)) throw new Error(`Row ${index + 2}: conflicting details for ${email}`);
    if (v.tool) {
      if (!["claude-code", "codex", "cursor", "opencode", "copilot", "other"].includes(v.tool.toLowerCase())) throw new Error(`Row ${index + 2}: unsupported tool name`);
      if (v.currency && !["USD", "JPY", "EUR", "GBP"].includes(v.currency.toUpperCase())) throw new Error(`Row ${index + 2}: currency must be USD, JPY, EUR or GBP`);
      if (p.assignments.some(a => a.tool === v.tool.toLowerCase())) throw new Error(`Row ${index + 2}: duplicate tool for ${email}`);
      const amount = v.monthly_amount === "" ? null : Number(v.monthly_amount);
      if (amount !== null && (!Number.isFinite(amount) || amount < 0 || amount > 1_000_000_000)) throw new Error(`Row ${index + 2}: invalid monthly amount`);
      p.assignments.push({ tool: v.tool.toLowerCase(), plan: v.plan, allocated_model: v.allocated_model, monthly_amount: amount, currency: v.currency.toUpperCase() || "USD" });
    } else if (v.plan || v.allocated_model || v.monthly_amount) throw new Error(`Row ${index + 2}: contract fields require a tool`);
    people.set(email, p);
  }
  if (!people.size || people.size > 1000) throw new Error("Import 1–1000 people at a time");
  return [...people.values()];
}
export function contractTotals(people: InventoryPerson[]): [string, number][] {
  const totals = new Map<string, number>();
  for (const p of people) for (const a of p.assignments) if (a.monthly_amount !== null) totals.set(a.currency, (totals.get(a.currency) ?? 0) + a.monthly_amount);
  return [...totals].sort(([a], [b]) => a.localeCompare(b));
}
