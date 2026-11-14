// Built-UI workflow checks. Synthetic API fixtures; no agent settings or paid APIs.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import path from "node:path";
const puppeteer = createRequire(
  new URL("../../desktop/package.json", import.meta.url),
)("puppeteer-core");
const dist = fileURLToPath(new URL("../dist/", import.meta.url));
const out =
  process.env.IMPROVEMENTS_UI_OUTPUT || "/tmp/kikimimi-improvements-ui";
await mkdir(out, { recursive: true });
const server = createServer(async (req, res) => {
  try {
    const route = new URL(req.url, "http://localhost").pathname;
    const file = route.startsWith("/assets/")
      ? path.join(dist, "assets", path.basename(route))
      : path.join(dist, "index.html");
    const bytes = await readFile(file);
    res.setHeader(
      "content-type",
      file.endsWith(".js")
        ? "text/javascript"
        : file.endsWith(".css")
          ? "text/css"
          : file.endsWith(".svg")
            ? "image/svg+xml"
            : "text/html",
    );
    res.end(bytes);
  } catch {
    res.writeHead(404).end();
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
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
  page.on("pageerror", (error) => errors.push(error.message));
  await page.setViewport({ width: 1440, height: 1100 });
  let mode = "candidate";
  let marks = [];
  let healthFailure = false;
  const now = new Date();
  const date = (daysAgo) =>
    new Date(now.getTime() - daysAgo * 86400000).toISOString().slice(0, 10);
  await page.setRequestInterception(true);
  page.on("request", (req) => {
    const url = new URL(req.url());
    const p = url.pathname;
    if (!p.startsWith("/web/")) return req.continue();
    let data = { columns: [], rows: [] },
      status = 200;
    if (p === "/web/me")
      data = { local: true, data_source: mode === "s3" ? "s3" : "local" };
    else if (p === "/web/collection-health") {
      if (healthFailure) {
        status = 503;
        data = { error: "unavailable" };
      } else
        data = {
          checked_at_ms: Date.now(),
          collector: "running",
          snapshot_fresh: true,
          heartbeat_at_ms: Date.now(),
          last_event_ts: null,
          last_flush_at_ms: null,
          spool_pending: 0,
          issues: [],
          cloud: { status: "off", pending: null, last_push_at: null },
          s3: { status: "off", pending: null, last_push_at: null },
        };
    } else if (p === "/web/q/filter-options")
      data = { this_host_id: "fixture", devices: [], repositories: [] };
    else if (p === "/web/q/patterns" && (mode === "candidate" || mode === "s3"))
      data.rows = [
        ["retry_spiral", "Read", 2, 6, null, 0, 2, null, date(12), date(5)],
      ];
    else if (p === "/web/q/pattern-hits")
      data.rows = [
        [
          date(5),
          "fixture-session",
          now.getTime() - 5 * 86400000,
          now.getTime() - 5 * 86400000 + 100,
          3,
          null,
          "{}",
        ],
      ];
    else if (p === "/web/q/pattern-timeline")
      data.rows =
        mode === "waiting"
          ? []
          : [
              [date(12), 5, 3, 60, 9, null],
              [date(11), 5, 2, 40, 6, null],
              [date(5), 5, 0, 0, 0, null],
              [date(4), 5, 1, 20, 3, null],
            ];
    else if (p === "/web/marks" && req.method() === "POST") {
      const body = JSON.parse(req.postData());
      assert.equal(url.searchParams.get("repository"), null);
      data = { ...body, id: "change-1", created_at: new Date().toISOString() };
      marks.push(data);
    } else if (p === "/web/marks") data = { marks: mode === "s3" ? [] : marks };
    else if (p.startsWith("/web/marks/") && req.method() === "DELETE") {
      marks = [];
      data = { deleted: "change-1" };
    }
    void req.respond({
      status,
      contentType: "application/json",
      body: JSON.stringify(data),
    });
  });
  const base = `http://127.0.0.1:${server.address().port}`;
  await page.goto(base + "/patterns", { waitUntil: "networkidle0" });
  await page.waitForFunction(() =>
    document.body.innerText.includes("Collector running"),
  );
  assert.match(
    await page.$eval(".collection-health", (el) => el.innerText),
    /Not observed/,
  );
  await page.click(".row-clickable");
  await page.waitForSelector("#mark-note");
  await page.waitForSelector('a[href="/sessions/fixture-session"]');
  await page.$eval(
    "#mark-dt",
    (el, value) => {
      const setter = Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      ).set;
      setter.call(el, value);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    },
    date(7),
  );
  await page.type("#mark-note", "Renewed the tool credentials");
  await page.click('.mark-form button[type="submit"]');
  await page.waitForSelector(".tracked-change");
  await page.waitForFunction(() =>
    document.body.innerText.includes("Lower observed recurrence rate"),
  );
  assert.doesNotMatch(
    await page.$eval(".mark-list", (el) => el.innerText),
    /resolved|improved/,
  );
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({
    path: path.join(out, "workflow.png"),
    fullPage: true,
  });
  mode = "gone";
  await page.reload({ waitUntil: "networkidle0" });
  await page.waitForSelector(".tracked-change");
  assert.equal(await page.$(".row-clickable"), null);
  await page.click(".tracked-change");
  await page.waitForFunction(() =>
    document.body.innerText.includes("Lower observed recurrence rate"),
  );
  mode = "waiting";
  await page.reload({ waitUntil: "networkidle0" });
  await page.click(".tracked-change");
  await page.waitForFunction(() =>
    document.body.innerText.includes("Waiting for activity after the change"),
  );
  healthFailure = true;
  await page.reload({ waitUntil: "networkidle0" });
  await page.click(".tracked-change");
  await page.waitForFunction(() =>
    document.body.innerText.includes(
      "Collection must be checked before interpreting results",
    ),
  );
  assert.match(
    await page.$eval(".collection-health", (el) => el.innerText),
    /status is unavailable/,
  );
  healthFailure = false;
  await page.evaluate(() =>
    localStorage.setItem("kikimimi.ui-language.v1", "ja"),
  );
  await page.reload({ waitUntil: "networkidle0" });
  await page.click(".tracked-change");
  await page.waitForFunction(() =>
    document.body.innerText.includes("対応後の利用を待っています"),
  );
  await page.setViewport({ width: 390, height: 844 });
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({
    path: path.join(out, "mobile-ja.png"),
    fullPage: true,
  });
  assert.ok(
    await page.$eval(
      ".mark-form",
      (el) => el.getBoundingClientRect().width <= 390,
    ),
  );
  mode = "s3";
  await page.evaluate(() =>
    localStorage.setItem("kikimimi.ui-language.v1", "en"),
  );
  await page.reload({ waitUntil: "networkidle0" });
  assert.match(
    await page.$eval(".collection-health", (el) => el.innerText),
    /S3 snapshot/,
  );
  await page.click(".row-clickable");
  await page.waitForFunction(() =>
    document.body.innerText.includes("S3 exports are read-only"),
  );
  assert.equal(await page.$(".mark-form"), null);
  assert.deepEqual(errors, []);
  console.log(
    `PASS: evidence → change → follow-up, disappearing candidates, insufficient data, health failures, Japanese and mobile. Screenshots: ${out}`,
  );
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
