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
  await page.setRequestInterception(true);
  page.on("request", (req) => {
    const p = new URL(req.url()).pathname;
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
    req.respond({
      status,
      contentType: "application/json",
      body: JSON.stringify(data),
    });
  });
  await page.goto(base + "/cost", { waitUntil: "networkidle0" });
  await page.waitForSelector("[data-cost=payment]");
  const selects = await page.$$eval("select", nodes => nodes.map(node => {
    const style = getComputedStyle(node);
    return { shared: node.dataset.slot === "select", right: style.paddingRight, appearance: style.appearance, image: style.backgroundImage };
  }));
  assert.ok(selects.length >= 4);
  for (const select of selects) {
    assert.equal(select.shared, true, "Every select uses NativeSelect");
    assert.equal(select.right, "40px", "Reserve space for the common arrow");
    assert.equal(select.appearance, "none");
    assert.notEqual(select.image, "none");
  }

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
  await page.select(".cost-controls select", "api");
  await page.click(".cost-opportunity button");
  await page.waitForSelector(".cost-plan");
  await page.select(".cost-controls select", "subscription");
  assert.equal(await page.$(".cost-opportunity"), null);
  assert.equal(
    await page.$eval("[data-cost=payment]", (e) => e.textContent),
    "Not connected",
  );
  async function fill(id, value) {
    await page.locator(id).fill(value);
    assert.equal(await page.$eval(id, (e) => e.value), value, id);
  }
  const verdict = () =>
    page.$eval("[data-comparison-verdict] h3", (e) => e.textContent);
  await fill("#compare-current-price", "100");
  await fill("#compare-proposed-price", "80");
  await page.click("#compare-scope");
  assert.match(await verdict(), /pending/);
  assert.match(
    await page.$eval("[data-subscription-verdict]", (e) => e.textContent),
    /Change plan/,
  );
  await fill("#compare-current-price", "10");
  assert.match(await verdict(), /Lowest cost.*Keep subscription/);
  await fill("#compare-current-price", "100");
  await page.evaluate(() => {
    const create = URL.createObjectURL;
    URL.createObjectURL = (blob) => {
      window.comparisonReport = blob.text();
      return create(blob);
    };
    document.addEventListener(
      "click",
      (event) => {
        if (event.target instanceof HTMLAnchorElement && event.target.download)
          event.preventDefault();
      },
      true,
    );
  });
  await page.$eval(".billing-comparison header button", e => e.scrollIntoView({ block: "center" }));
  await page.click(".billing-comparison header button");
  await page.waitForFunction(() => window.comparisonReport !== undefined);
  assert.match(
    await page.evaluate(() => window.comparisonReport),
    /Move to API: ≥ \$24.00/,
  );
  scenario = "complete";
  await page.select(".cost-heading select", "7");
  await page.waitForFunction(
    () =>
      document.querySelector("[data-cost=equivalent]")?.textContent ===
      "$24.00",
  );
  assert.equal(
    await page.$eval("#compare-current-price", (e) => e.value),
    "100",
  );
  assert.equal(await page.$eval("#compare-scope", (e) => e.checked), false);
  await page.click("#compare-scope");
  assert.match(await verdict(), /Lowest cost.*Change plan/);
  await page.select(".cost-heading select", "30");
  await page.waitForFunction(() =>
    document
      .querySelector("[data-comparison-monthly=api]")
      ?.textContent.includes("$24.00"),
  );
  assert.match(await verdict(), /pending/);
  await page.click("#compare-scope");
  assert.match(await verdict(), /Lowest cost.*Move to API/);
  await fill("#compare-months", "1");
  await fill("#compare-api-transition", "100");
  assert.match(await verdict(), /Lowest cost.*Change plan/);
  await fill("#compare-current-price", "20");
  assert.match(await verdict(), /Lowest cost.*Keep subscription/);
  await fill("#compare-current-price", "120");
  await page.select("#compare-current-period", "annual");
  await fill("#compare-current-seats", "2");
  await fill("#compare-current-extras", "5");
  assert.match(
    await page.$eval("[data-comparison-monthly=current]", (e) => e.textContent),
    /\$25.00/,
  );
  await page.select("#compare-currency", "JPY");
  assert.equal(await page.$eval("#compare-current-price", (e) => e.value), "");
  assert.equal(await page.$eval("#compare-scope", (e) => e.checked), false);
  await fill("#compare-current-price", "5000");
  await fill("#compare-proposed-price", "4000");
  await fill("#compare-fx", "150");
  await page.click("#compare-scope");
  assert.match(await verdict(), /Lowest cost.*Move to API/);
  await fill("#compare-usage", "200");
  assert.match(await verdict(), /Lowest cost.*Change plan/);
  await fill("#compare-usage", "100");
  await (
    await page.$(".billing-comparison")
  ).screenshot({
    path: `/tmp/kikimimi-billing-${cloud ? "cloud" : "desktop"}.png`,
  });
  await page.select(".cost-controls select", "unknown");
  await page.screenshot({
    path: `/tmp/kikimimi-cost-${cloud ? "cloud" : "local"}-coverage.png`,
    fullPage: true,
  });
  await page.setViewport({ width: 390, height: 844 });
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
  );
  scenario = "error";
  await page.select(".cost-heading select", "7");
  await page.waitForSelector("[role=alert]");
  assert.match(await verdict(), /pending/);
  assert.match(
    await page.$eval("[data-comparison-monthly=api]", (e) => e.textContent),
    /—/,
  );
  scenario = "empty";
  await page.select(".cost-heading select", "90");
  await page.waitForSelector("[data-cost=equivalent]");
  assert.equal(
    await page.$eval("[data-cost=equivalent]", (e) => e.textContent),
    "—",
  );
  assert.match(await verdict(), /pending/);
  if (cloud) {
    role = "member";
    await page.reload({ waitUntil: "networkidle0" });
    assert.equal(await page.$(".cost-accounting"), null);
    assert.match(await page.$eval("body", (e) => e.textContent), /Your teams/);
  }
  role = "admin";
  scenario = "complete";
  await page.evaluate(() =>
    localStorage.setItem("kikimimi.ui-language.v1", "ja"),
  );
  await page.reload({ waitUntil: "networkidle0" });
  await page.waitForSelector("#billing-compare-title");
  assert.match(
    await page.$eval("#billing-compare-title", (e) => e.textContent),
    /どの支払い方が安いか/,
  );
  assert.match(await verdict(), /保留/);
  await fill("#compare-current-price", "100");
  await fill("#compare-proposed-price", "80");
  await page.click("#compare-scope");
  assert.match(await verdict(), /最安.*API移行/);
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
  );
  assert.deepEqual(errors, []);
  console.log(
    `PASS ${cloud ? "Cloud" : "local"}: partial/null/zero payment, request coverage, model details, three-way ranking, annual/seats/extras, migration costs, FX, usage, period reset, error/empty states, mobile layout${cloud ? ", member role guard" : ""}`,
  );
} finally {
  await browser.close();
  server.close();
}
