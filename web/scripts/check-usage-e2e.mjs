// Real CLI -> local snapshots -> Rust router -> built SPA -> Chrome.
// Only the provider boundary is synthetic; never reads real account credentials.
// Prerequisites: npm run build; cargo build -p kikimimi --bin kikimimi --example usage_e2e_server
// Run: node web/scripts/check-usage-e2e.mjs (Chrome on PATH, or CHROME_BIN).
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, mkdir, writeFile, readFile, readdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createInterface } from "node:readline";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const scratch = await mkdtemp(path.join(tmpdir(), "kikimimi-usage-e2e-"));
const cli = path.join(repo, "target/debug/kikimimi");
const bin = path.join(scratch, "bin");
const data = path.join(scratch, "data/events");
await mkdir(bin);
const env = { ...process.env, KIKIMIMI_DIR: scratch, PATH: `${bin}:${process.env.PATH}` };
const children = [];
const browserErrors = [];
const checks = [];
let socket;

function check(condition, label) {
  assert.ok(condition, label);
  checks.push(label);
  console.log(`ok ${label}`);
}
function run(args, input) {
  const result = spawnSync(cli, args, { env, input: input && JSON.stringify(input), encoding: "utf8", timeout: 35_000 });
  assert.equal(result.status, 0, result.stderr || String(result.error));
  return result.stdout;
}
async function until(fn, label, timeout = 15_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const result = await fn();
    if (result) return result;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out: ${label}`);
}
function launch(command, args) {
  const child = spawn(command, args, { env, stdio: ["ignore", "pipe", "pipe"] });
  children.push(child);
  return child;
}

try {
  // A real child process exercises initialize/read/exit over stdio. Each explicit
  // profile supplies different limits; the fixture rejects malformed handshakes.
  await writeFile(path.join(bin, "codex"), `#!/usr/bin/env node
import { createInterface } from 'node:readline';
import { readFileSync } from 'node:fs';
let initialized = false;
let ready = false;
for await (const line of createInterface({ input: process.stdin })) {
  const message = JSON.parse(line);
  if (message.method === 'initialize') {
    initialized = true;
    console.log(JSON.stringify({ id: message.id, result: {} }));
  } else if (message.method === 'initialized' && initialized) {
    ready = true;
  } else if (message.method === 'account/rateLimits/read' && ready) {
    console.log(JSON.stringify({ id: message.id, result: JSON.parse(readFileSync(process.env.CODEX_HOME + '/limits.json', 'utf8')) }));
  } else process.exit(1);
}
`, { mode: 0o700 });
  // Explicitly mark the extensionless fixture as an ES module.
  await writeFile(path.join(bin, "package.json"), '{"type":"module"}');
  const server = launch(path.join(repo, "target/debug/examples/usage_e2e_server"), [data]);
  let base;
  createInterface({ input: server.stdout }).on("line", (line) => { if (line.startsWith("http://")) base = line; });
  server.stderr.on("data", (chunk) => process.stderr.write(chunk));
  await until(() => base, "Rust server startup");
  check((await fetch(`${base}/web/usage`)).status === 401, "usage endpoint rejects unauthenticated requests");

  const chromeDir = path.join(scratch, "chrome");
  const chrome = launch(process.env.CHROME_BIN || "google-chrome", ["--headless=new", "--no-sandbox", "--disable-dev-shm-usage", "--remote-debugging-port=0", `--user-data-dir=${chromeDir}`, "about:blank"]);
  let browserEndpoint;
  createInterface({ input: chrome.stderr }).on("line", (line) => {
    const match = line.match(/DevTools listening on (ws:\/\/\S+)/);
    if (match) browserEndpoint = match[1];
  });
  await until(() => browserEndpoint, "Chrome startup");
  const port = new URL(browserEndpoint).port;
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  socket = new WebSocket(targets.find((target) => target.type === "page").webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let id = 0;
  const pending = new Map();
  socket.onmessage = ({ data }) => {
    const reply = JSON.parse(data);
    if (reply.method === "Runtime.exceptionThrown") browserErrors.push(reply.params.exceptionDetails);
    const entry = pending.get(reply.id);
    if (entry) {
      pending.delete(reply.id);
      clearTimeout(entry.timer);
      if (reply.error) entry.reject(new Error(JSON.stringify(reply.error)));
      else entry.resolve(reply.result);
    }
  };
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const messageId = ++id;
    const timer = setTimeout(() => { pending.delete(messageId); reject(new Error(`CDP timeout: ${method}`)); }, 15_000);
    pending.set(messageId, { resolve, reject, timer });
    socket.send(JSON.stringify({ id: messageId, method, params }));
  });
  const evaluate = async (expression) => {
    const result = await call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  const section = `document.querySelector('section[aria-labelledby="subscription-usage-title"]')`;
  const text = () => evaluate(`${section}?.innerText || ''`);
  const refresh = () => evaluate(`${section}.querySelector('button').click()`);
  const select = (value) => evaluate(`(() => { const select = ${section}.querySelector('select'); select.value = ${JSON.stringify(value)}; select.dispatchEvent(new Event('change', { bubbles: true })); })()`);
  await call("Runtime.enable");
  await call("Page.enable");
  await call("Emulation.setDeviceMetricsOverride", { width: 1440, height: 1100, deviceScaleFactor: 1, mobile: false });
  await call("Page.navigate", { url: `${base}/?t=usage-e2e-token` });
  await until(async () => (await text()).includes("No account usage recorded yet."), "empty state");
  check(true, "real local SPA renders account setup when no snapshots exist");

  const future = Math.floor(Date.now() / 1000) + 3600;
  run(["usage", "claude", "--account", "personal"], { rate_limits: { five_hour: { used_percentage: 42, resets_at: future } }, prompt: "SECRET-MUST-NOT-BE-STORED" });
  run(["usage", "claude", "--account", "work"], { rate_limits: { seven_day: { used_percentage: 78, resets_at: future } } });
  for (const [account, limits] of [
    ["work", { rateLimitsByLimitId: { codex: { primary: { usedPercent: 25, windowDurationMins: 300 } }, other: { secondary: { usedPercent: 61, windowDurationMins: 10080, resetsAt: future } } } }],
    ["personal", { rateLimits: null }],
  ]) {
    const profile = path.join(scratch, `codex-${account}`);
    await mkdir(profile);
    await writeFile(path.join(profile, "limits.json"), JSON.stringify(limits));
    run(["usage", "codex", "--account", account, "--profile", profile]);
  }
  const snapshots = JSON.parse(run(["usage"]));
  check(snapshots.length === 4, "real CLI records four separate provider/account pairs");
  check(!JSON.stringify(snapshots).includes("SECRET-MUST-NOT-BE-STORED"), "raw input is excluded from stored snapshots");
  await refresh();
  await until(() => evaluate(`${section}.querySelectorAll('article').length === 4`), "four account cards");
  check((await text()).includes("42.0% used") && (await text()).includes("25.0% used"), "Claude and Codex percentages reach the browser through the real API");
  check((await text()).includes("Usage unavailable") && (await text()).includes("Reset time unknown"), "missing windows and reset times are shown as unknown");
  check(await evaluate(`${section}.querySelectorAll('progress').length === 4`), "both Codex limit buckets are displayed");
  await select(JSON.stringify(["claude", "work"]));
  await until(() => evaluate(`${section}.querySelectorAll('article').length === 1`), "account filter");
  check(await evaluate(`${section}.querySelector('article h3').innerText === 'Claude · work'`), "account filter isolates Claude work from Codex work");
  run(["usage", "claude", "--account", "work"], { rate_limits: { seven_day: { used_percentage: 12, resets_at: future } } });
  await refresh();
  await until(async () => (await text()).includes("12.0% used"), "updated snapshot");
  check(!(await text()).includes("78.0% used"), "new usage replaces old usage without summing and preserves selection");
  // Advance browser time only: deterministic freshness/reset checks without
  // modifying snapshots or waiting an hour.
  await evaluate(`Date.now = () => ${Date.now() + 7_200_000}`);
  await refresh();
  await until(async () => (await text()).includes("May be outdated"), "stale observation");
  check((await text()).includes("Reset time passed — awaiting a new observation") && (await text()).includes("12.0% used at last observation"), "expired resets retain observed usage and show freshness warning");
  await select("");
  await until(() => evaluate(`${section}.querySelectorAll('article').length === 4`), "all accounts restored");
  const screenshot = await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: true });
  await writeFile(path.join(scratch, "desktop.png"), Buffer.from(screenshot.data, "base64"));
  await call("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  check(await evaluate(`${section}.getBoundingClientRect().right <= window.innerWidth`), "usage section fits a narrow viewport");
  const mobile = await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: true });
  await writeFile(path.join(scratch, "mobile.png"), Buffer.from(mobile.data, "base64"));
  check(browserErrors.length === 0, "no uncaught browser exceptions");
  // Verify bytes on disk too, not just the CLI's normalized output.
  for (const name of await readdir(path.join(data, "subscription-usage"))) {
    if (name.endsWith(".json")) assert.ok(!(await readFile(path.join(data, "subscription-usage", name), "utf8")).includes("SECRET-MUST-NOT-BE-STORED"));
  }
  await writeFile(path.join(scratch, "results.json"), JSON.stringify({ checks, providerBoundary: "synthetic", browserErrors }, null, 2));
  console.log(`PASS ${checks.length} E2E checks. Screenshots and results: ${scratch}`);
} finally {
  socket?.close();
  for (const child of children.reverse()) {
    child.kill("SIGTERM");
    await Promise.race([new Promise((resolve) => child.once("exit", resolve)), new Promise((resolve) => setTimeout(resolve, 1500))]);
    if (child.exitCode === null) child.kill("SIGKILL");
  }
}
