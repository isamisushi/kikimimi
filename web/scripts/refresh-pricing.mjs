// Fetch public official tables only. Never sends usage or credentials.
import { writeFile, rename } from "node:fs/promises";
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
  // Explicit allowlist: unreviewed/new/retired models never silently become recommendations.
  const claudeNames = [
    "Claude Fable 5.1",
    "Claude Fable 5",
    "Claude Opus 5",
    "Claude Opus 5.5",
    "Claude Opus 4.6",
    "Claude Sonnet 5.5",
    "Claude Sonnet 4.6",
    "Claude Haiku 5.5",
    "Claude Haiku 4.5",
  ];
  for (const name of claudeNames) {
    const rows = anthropic
      .split("\n")
      .filter((l) => l.startsWith("| " + name + " "))
      .map(cells);
    if (
      rows.length !== (name === "Claude Haiku 5.5" ? 2 : 1) ||
      rows.some((r) => r.length !== 6)
    )
      throw new Error(`Claude table changed: ${name}`);
    models.push({
      id: name.toLowerCase().replaceAll(" ", "-").replaceAll(".", "-"),
      name,
      provider: "anthropic",
      variants: rows.flatMap((r) =>
        [2, 3].map((write) => ({
          input: money(r[1]),
          output: money(r[5]),
          read: money(r[4]),
          write: money(r[write]),
        })),
      ),
    });
  }
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
  for (const id of [
    "gpt-6-astra",
    "gpt-6.1-sol",
    "gpt-6.1-terra",
    "gpt-6-luna",
    "gpt-6-sol",
    "gpt-5.4-mini",
  ]) {
    const rows = standard
      .split("\n")
      .filter((l) => l.startsWith("| " + id + " |"))
      .map(cells);
    if (rows.length !== 1 || ![4, 11].includes(rows[0].length))
      throw new Error(`OpenAI table changed: ${id}`);
    const r = rows[0];
    const variants =
      r.length === 4
        ? [
            {
              input: money(r[1]),
              output: money(r[2]),
              read: money(r[3]),
              write: null,
            },
          ]
        : [1, 6].flatMap((i) =>
            [3, 4].map((w) => ({
              input: money(r[i]),
              output: money(r[i + 1]),
              read: money(r[i + 2]),
              write: money(r[i + w]),
            })),
          );
    models.push({ id, name: id, provider: "openai", variants });
  }
  return models;
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
  const output = new URL("../src/cost/pricing.json", import.meta.url);
  const temporary = new URL("../src/cost/pricing.json.tmp", import.meta.url);
  await writeFile(
    temporary,
    JSON.stringify(
      { fetchedAt: new Date().toISOString(), sources, models },
      null,
      2,
    ) + "\n",
  );
  await rename(temporary, output);
  console.log(
    `Updated ${models.length} models from official standard/global USD tables.`,
  );
}
