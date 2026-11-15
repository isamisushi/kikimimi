// Fetch public official tables only. Never sends usage or credentials.
import { readFile, writeFile, rename } from "node:fs/promises";
import { fileURLToPath } from "node:url";

export const sources = {
  anthropic: "https://platform.claude.com/docs/en/about-claude/pricing",
  openai: "https://developers.openai.com/api/docs/pricing",
};
const money = (cell) => {
  const match = cell.match(/^\$(\d+(?:\.\d+)?)(?:\s|$)/);
  if (!match) throw new Error(`Unrecognized price: ${cell}`);
  return Number(match[1]);
};
const cells = (line) =>
  line
    .split("|")
    .slice(1, -1)
    .map((s) => s.trim());
export function parsePrices(anthropic, openai) {
  const models = [];
  anthropic = anthropic.split("## Model pricing")[1]?.split("\n## ")[0] ?? "";
  const header = anthropic.split("\n").find((l) => l.startsWith("| Model"));
  if (
    !header ||
    cells(header).join("|") !==
      "Model|Base input tokens|5m cache writes|1h cache writes|Cache hits and refreshes|Output tokens"
  )
    throw new Error("Claude price columns changed");
  for (const line of anthropic.split("\n").filter(l => /^\| Claude /.test(l))) {
    const r = cells(line);
    if (r.length !== 6) throw new Error("Unexpected Claude rate columns");
    const name = r[0].split(" (")[0];
    const id = name.toLowerCase().replaceAll(" ", "-").replaceAll(".", "-");
    let model = models.find(m => m.id === id);
    if (!model) models.push(model = {id, name, provider: "anthropic", variants: []});
    for (const write of [2, 3]) model.variants.push({
      input: money(r[1]), output: money(r[5]), read: money(r[4]), write: money(r[write]),
    });
  }
  if (!models.length) throw new Error("Empty Claude pricing table");
  const standard = openai
    .split("### Standard pricing data")[1]
    ?.split("### Batch pricing data")[0];
  if (!standard) throw new Error("OpenAI standard table missing");
  const headers = standard
    .split("\n")
    .filter((l) => l.startsWith("| Model"))
    .map((l) => cells(l).join("|"));
  if (
    headers.length !== 2 ||
    headers[0] !==
      "Model|Short context input|Short context output|Short context cached input|Short context 30m write|Short context 12h write|Long context input|Long context output|Long context cached input|Long context 30m write|Long context 12h write" ||
    headers[1] !== "Model|Input|Output|Cached input"
  )
    throw new Error("OpenAI price columns changed");
  const optional = cell => cell === "-" ? null : money(cell);
  const openaiRows = standard.split("\n").filter(l => l.startsWith("| ") && !/^\| (Model \||---)/.test(l));
  if (!openaiRows.length) throw new Error("Empty OpenAI pricing table");
  for (const line of openaiRows) {
    const r = cells(line);
    const id = r[0].split(" (")[0];
    if (!/^[a-z0-9][a-z0-9.-]*$/.test(id) || ![4, 11].includes(r.length))
      throw new Error(`Unexpected OpenAI rate row: ${r[0]}`);
    const variants =
      r.length === 4
        ? [
            {
              input: money(r[1]),
              output: money(r[2]),
              read: optional(r[3]),
              write: null,
            },
          ]
        : [1, 6].flatMap((i) =>
            [3, 4].map((w) => ({
              input: money(r[i]),
              output: money(r[i + 1]),
              read: optional(r[i + 2]),
              write: optional(r[i + w]),
            })),
          );
    models.push({ id, name: id, provider: "openai", variants });
  }
  if (new Set(models.map(m => m.id)).size !== models.length) throw new Error("Duplicate model ID");
  return models.sort((a, b) => a.id.localeCompare(b.id, "en"));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const docs = await Promise.all(
    Object.values(sources).map(async (url) => {
      const res = await fetch(url + ".md", {
        signal: AbortSignal.timeout(30_000),
      });
      if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
      return res.text();
    }),
  );
  const models = parsePrices(...docs);
  const output = new URL("../src/cost/price-history.json", import.meta.url);
  const history = JSON.parse(await readFile(output, "utf8"));
  const next = recordPrices(history, models, new Date().toISOString());
  const temporary = new URL(output.href + ".tmp");
  await writeFile(temporary, JSON.stringify(next, null, 2) + "\n");
  await rename(temporary, output);
}

export function recordPrices(history, models, fetchedAt) {
  const effectiveFrom = fetchedAt.slice(0, 10);
  const previous = history.versions.at(-1);
  if (fetchedAt < history.fetchedAt) throw new Error("Pricing clock moved backwards");
  const versions = [...history.versions];
  if (JSON.stringify(previous.models) !== JSON.stringify(models)) {
    if (effectiveFrom === previous.effectiveFrom) throw new Error("Multiple changes on one UTC day require review");
    versions.push({ effectiveFrom, models });
  }
  return { fetchedAt, versions };
}
