const { invoke } = window.__TAURI__.core;
const $ = (id) => document.getElementById(id);
let busy = false;
let ready = false;
let updates = { configured: false, available_version: null, phase: 'idle' };

function updateBusy() { return ['checking', 'downloading', 'installing', 'restarting'].includes(updates.phase); }

function controls() {
  const blocked = busy || updateBusy();
  for (const button of document.querySelectorAll('button')) button.disabled = blocked;
  $('enable').disabled = blocked || !$('consent').checked;
  $('dashboard').disabled = blocked || !ready;
  $('check-updates').disabled = blocked || !updates.configured;
  $('install-update').disabled = blocked || !updates.configured || !updates.available_version;
  $('auto-update').disabled = blocked || !updates.configured;
}

async function refreshUpdates() {
  updates = await invoke('update_status');
  $('auto-update').checked = updates.auto_update;
  const messages = { checking: 'Checking for updates…', downloading: 'Downloading and verifying update…', installing: 'Installing update…', restarting: 'Restarting…', current: 'You’re up to date.' };
  $('update-status').textContent = !updates.configured ? `v${updates.current_version} · Updates are unavailable in this development build.`
    : updates.error ? `Update failed: ${updates.error}`
    : messages[updates.phase] || (updates.available_version ? `v${updates.available_version} is available (installed: v${updates.current_version}).` : `Installed: v${updates.current_version}`);
  controls();
}

async function refresh() {
  const state = await invoke('status');
  ready = Boolean(state.dashboard_url) && state.duckdb_available;
  $('status').textContent = state.running ? 'Collector is running' : 'Collection is not running';
  $('details').textContent = state.web_error
    || (!state.duckdb_available ? 'The bundled query engine is unavailable. Reinstall the application.'
      : state.service_installed ? 'Collection starts automatically when you log in.'
        : 'Enable collection below to finish setup.');
  controls();
  await refreshUpdates();
}

async function act(work) {
  if (busy) return;
  busy = true;
  $('message').textContent = '';
  controls();
  try { await work(); }
  catch (error) { $('message').textContent = String(error) || 'Operation failed. Refresh status and retry.'; }
  finally { busy = false; controls(); }
}

$('consent').addEventListener('change', controls);
$('auto-update').addEventListener('change', () => act(async () => {
  try { await invoke('set_auto_update', { enabled: $('auto-update').checked }); }
  finally { await refreshUpdates(); }
}));
for (const [id, command] of [['check-updates', 'check_updates'], ['install-update', 'install_update']]) {
  $(id).addEventListener('click', () => act(async () => {
    try { await invoke(command); } finally { await refreshUpdates(); }
  }));
}
$('refresh').addEventListener('click', () => act(refresh));
$('enable').addEventListener('click', () => act(async () => {
  $('message').textContent = 'Setting up collection…';
  await invoke('enable');
  $('message').textContent = 'Collection is enabled. Restart existing agent sessions to load the new settings.';
  $('consent').checked = false;
  await refresh();
}));
$('dashboard').addEventListener('click', () => act(() => invoke('open_dashboard')));
$('disconnect').addEventListener('click', () => $('disconnect-dialog').showModal());
$('cancel-disconnect').addEventListener('click', () => $('disconnect-dialog').close());
$('confirm-disconnect').addEventListener('click', () => act(async () => {
  $('disconnect-dialog').close();
  await invoke('disconnect');
  await refresh();
  $('message').textContent = 'Disconnected. Your recorded data has been kept.';
}));
act(refresh);
setInterval(() => { if (!busy && !$('disconnect-dialog').open) act(refresh); }, 15000);
setInterval(() => { refreshUpdates().catch(() => {}); }, 2000);
