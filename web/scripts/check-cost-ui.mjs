// Isolated browser smoke test against built UI, with synthetic usage and roles.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import path from "node:path";
const ownRequire = createRequire(new URL("../package.json", import.meta.url));
let puppeteer;
try {
  puppeteer = ownRequire("puppeteer-core");
} catch {
  puppeteer = createRequire(
    new URL("../../desktop/package.json", import.meta.url),
  )("puppeteer-core");
}
const cloud = process.argv.includes("--cloud");
const dist = fileURLToPath(new URL("../dist/", import.meta.url));
const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url, "http://localhost");
    const asset = url.pathname.startsWith("/assets/");
    const file = asset
      ? path.join(
          dist,
          path.basename("assets") + "/" + path.basename(url.pathname),
        )
      : path.join(dist, "index.html");
    const content = await readFile(file);
    res.setHeader(
      "Content-Type",
      file.endsWith(".js")
        ? "text/javascript"
        : file.endsWith(".css")
          ? "text/css"
          : file.endsWith(".svg")
            ? "image/svg+xml"
            : "text/html",
    );
    res.end(content);
  } catch {
    res.writeHead(404);
    res.end();
  }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}`;
const browser = await puppeteer.launch({
  executablePath:
    process.env.CHROME_BIN ||
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  headless: true,
  args: ["--no-sandbox"],
});
try {
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  let scenario = "partial",
    role = "admin";
  await page.setViewport({ width: 1440, height: 1100 });
  const bundled = JSON.parse(await readFile(new URL("../src/cost/price-history.json", import.meta.url), "utf8"));
  const nextDate = new Date(Date.parse(bundled.fetchedAt) + 86400000).toISOString();
  const newDay = nextDate.slice(0, 10);
  const nextHistory = {...bundled, fetchedAt: nextDate, versions: [...bundled.versions,
    {effectiveFrom: newDay, models: [{id:"claude-opus-5-5", name:"Claude Opus 5.5", provider:"anthropic",
      variants:[{input: 7, output: 31, read: 1, write: 2}]}]}]};
  let usageDay = "2026-11-10", feedOnline = true;
  await page.setRequestInterception(true);
  page.on("request", (req) => {
    const url = new URL(req.url());
    if (url.hostname === "raw.githubusercontent.com")
      return feedOnline ? req.respond({status:200, contentType:"application/json",
        headers:{"Access-Control-Allow-Origin":"*"}, body:JSON.stringify(nextHistory)}) : req.abort();
    const p = url.pathname;
    if (!p.startsWith("/web/")) return req.continue();
    let data = { rows: [], columns: [] };
    let status = 200;
    if (p === "/web/me")
      data = {
        local: !cloud,
        data_source: "local",
        email: "fixture@example.com",
        org_id: "test",
        active_org: "test",
        orgs: [{ slug: "test", name: "Test team", kind: "team", role }],
        subscription_usage: !cloud,
      };
    else if (p === "/web/q/filter-options")
      data = { this_host_id: "test", devices: [], repositories: [] };
    else if (p === "/web/usage") data = [];
    else if (p === "/web/self/devices") data = { devices: [] };
    else if (p === "/web/q/models") {
      if (scenario === "error") {
        status = 500;
        data = { error: "fixture failure" };
      } else
        data = {
          days: 30,
          models: {
            columns: [],
            rows:
              scenario === "empty"
                ? []
                : [
                    [
                      "claude-opus-5-5",
                      "high",
                      10,
                      0,
                      1,
                      0,
                      0,
                      9000000,
                      5000000,
                      3000000,
                      0,
                      0,
                      10,
                      3,
                      4,
                      1000000,
                      1000000,
                      0,
                      0,
                    ],
                    [
                      "unlisted",
                      "high",
                      2,
                      0,
                      1,
                      0,
                      0,
                      100,
                      100,
                      0,
                      0,
                      0,
                      null,
                      0,
                      2,
                      100,
                      100,
                      0,
                      0,
                    ],
                  ],
          },
          daily: { rows: [], columns: [] },
        };
    }
    if (p === "/web/q/models" && scenario === "complete") {
      data.models.rows = [
        [
          "claude-opus-5-5",
          "high",
          10,
          0,
          1,
          0,
          0,
          1e6,
          1e6,
          0,
          0,
          0,
          24,
          10,
          10,
          1e6,
          1e6,
          0,
          0,
        ],
      ];
    }
    if (p === "/web/q/models" && data.models) {
      for (const row of data.models.rows)
        row[19] = JSON.stringify([[usageDay, ...row.slice(14, 19)]]);
    }
    req.respond({
      status,
      contentType: "application/json",
      body: JSON.stringify(data),
    });
  });
  await page.goto(base + "/models", { waitUntil: "networkidle0" });
  await page.waitForSelector("[data-cost=payment]");
  assert.equal(
    await page.$eval("[data-cost=payment]", (e) => e.textContent),
    "Not connected",
  );
  assert.equal(
    await page.$eval("[data-cost=reported]", (e) => e.textContent),
    "$10.00",
  );
  assert.equal(
    await page.$eval("[data-cost=equivalent]", (e) => e.textContent),
    "$24.00",
  );
  assert.match(
    await page.$eval(".cost-coverage-note", (e) => e.textContent),
    /Requests not priced: 8 \/ 12/,
  );
  assert.match(
    await page.$eval(".cost-coverage-note", (e) => e.textContent),
    /Requests without reported cost: 9 \/ 12/,
  );
  await page.click(".cost-accounting summary");
  assert.equal(
    await page.$$eval(".cost-coverage-table tbody tr", (es) => es.length),
    2,
  );
  await page.waitForFunction(() => localStorage.getItem("kikimimi.pricing.history.v1") !== null);
  usageDay = newDay;
  feedOnline = false;
  await page.reload({waitUntil: "networkidle0"});
  assert.equal(await page.$eval("[data-cost=equivalent]", e => e.textContent), "$38.00");
  usageDay = "2026-11-10";
  await page.reload({waitUntil: "networkidle0"});
  assert.equal(await page.$eval("[data-cost=equivalent]", e => e.textContent), "$24.00");
  assert.deepEqual(errors, []);
  console.log(
    "PASS: model cost and coverage breakdown",
  );
} finally {
  await browser.close();
  server.close();
}
