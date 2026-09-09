const { invoke } = window.__TAURI__.core;
const $ = (id) => document.getElementById(id);
let state = null;
let view = 'loading';
let updates = { configured: false, phase: 'idle', available_version: null };
let collectionAction = null;
let updateAction = null;
let statusError = false;
let message = null;
let statusRequest = null;
let updateRequest = null;
let navigation = Promise.resolve();
let navigationId = 0;
let browseOnly = false;
let sourceKind = localStorage.getItem('data-source') || (localStorage.getItem('dashboard-source') === 'cloud' ? 'cloud' : 'local');
let sourceBusy = false;
let lastDashboard = localStorage.getItem('dashboard-source') === 'cloud' ? 'cloud' : 'dashboard';

// Workspace identity and its viewing preference are persisted together.
let workspace = 'personal';
let preferences = {};
let cloudOrgs = [];
let cloudRequest = null;
let sourceRecovery = false;
let signingIn = false;
let collectionSwitch = false;
let collectionReview = null;
try {
  const saved = JSON.parse(localStorage.getItem('workspaces-v1') || 'null');
  if (saved && typeof saved.preferences === 'object' && saved.preferences) {
    preferences = saved.preferences;
    if (typeof saved.active === 'string' && (['personal', 's3-team'].includes(saved.active) || saved.active.startsWith('cloud:'))) workspace = saved.active;
  }
} catch { /* A damaged preference must not prevent opening local history. */ }
if (!preferences.personal) preferences.personal = { kind: ['local', 's3', 'cloud'].includes(sourceKind) ? sourceKind : 'local' };
const isCloudTeam = (id) => id.startsWith('cloud:');
function preference(id) {
  const saved = preferences[id];
  if (isCloudTeam(id)) return { kind: 'cloud' };
  if (id === 's3-team') return { ...saved, kind: 's3' };
  return saved && ['local', 's3', 'cloud'].includes(saved.kind) ? saved : { kind: 'local' };
}
sourceKind = preference(workspace).kind;
lastDashboard = sourceKind === 'cloud' ? 'cloud' : 'dashboard';
function persistWorkspace() {
  localStorage.setItem('workspaces-v1', JSON.stringify({ active: workspace, preferences }));
  localStorage.setItem('data-source', sourceKind);
  localStorage.setItem('dashboard-source', lastDashboard);
}
function workspaceName(id) {
  if (id === 'personal') return 'Personal';
  if (id === 's3-team') return 'Team · S3';
  return cloudOrgs.find(org => `cloud:${org.slug}` === id)?.name || 'Cloud team';
}
function renderWorkspace() {
  const choices = [['personal', 'Personal'], ...cloudOrgs.filter(org => org.kind === 'team').map(org => [`cloud:${org.slug}`, org.name]), ['s3-team', 'Team · S3']];
  if (!choices.some(([id]) => id === workspace)) choices.splice(1, 0, [workspace, workspaceName(workspace)]);
  for (const choice of choices) if (collectingWorkspaces().includes(choice[0])) choice[1] = `● ${choice[1]} · Collecting on this Mac`;
  $('workspace-collecting').hidden = !collectingWorkspaces().includes(workspace);
  const select = $('workspace');
  const signature = JSON.stringify(choices);
  if (select.dataset.choices !== signature) {
    select.replaceChildren(...choices.map(([value, name]) => new Option(name, value)));
    select.dataset.choices = signature;
  }
  select.value = workspace;
  select.disabled = sourceBusy || collectionSwitch;
  $('source-workspace').textContent = workspaceName(workspace);
  renderWorkspaceDetails();
}
async function refreshCloud(adopt = false) {
  if (cloudRequest) return cloudRequest;
  cloudRequest = (async () => {
    const session = await invoke('cloud_workspaces');
    cloudOrgs = Array.isArray(session?.orgs) ? session.orgs : [];
    if (adopt && !sourceBusy && session) {
      const active = cloudOrgs.find(org => org.slug === session.active_org);
      if (active) {
        workspace = active.kind === 'personal' ? 'personal' : `cloud:${active.slug}`;
        preferences[workspace] = { ...preferences[workspace], kind: 'cloud' };
        sourceKind = 'cloud'; lastDashboard = 'cloud'; signingIn = false;
        persistWorkspace();
        render();
      }
    }
    renderWorkspace();
    return session;
  })();
  try { return await cloudRequest; } finally { cloudRequest = null; }
}
async function openPreference(id, selection) {
  sourceRecovery = true;
  if (selection.kind === 'cloud') {
    const session = await refreshCloud();
    const org = cloudOrgs.find(org => id === 'personal' ? org.kind === 'personal' : `cloud:${org.slug}` === id);
    if (org) await invoke('select_cloud_workspace', { slug: org.slug });
    else if (isCloudTeam(id)) throw new Error('Sign in to Cloud and refresh your team list.');
    else if (session) throw new Error('Your Personal workspace is unavailable. Refresh your team list.');
    signingIn = !session;
    await invoke('show_cloud');
  } else {
    signingIn = false;
    await invoke('set_source', { selection });
    await invoke('show_dashboard');
  }
  sourceRecovery = false;
  message = null;
  workspace = id;
  preferences[id] = { ...preferences[id], ...selection };
  sourceKind = selection.kind;
  lastDashboard = sourceKind === 'cloud' ? 'cloud' : 'dashboard';
  browseOnly = true;
  view = lastDashboard;
  ++navigationId;
  persistWorkspace();
  render();
}
async function changeWorkspace(id, keepSettings = true) {
  if (sourceBusy || collectionSwitch) return;
  if (id === 'signin') {
    $('workspace').value = workspace;
    await navigate('connection');
    sourceBusy = true; render();
    try {
      await refreshCloud();
      if (!cloudOrgs.length) {
        signingIn = true;
        await invoke('show_cloud');
        view = 'cloud';
        notice('Sign in to choose a Cloud workspace.');
      } else notice('Team list refreshed. Choose a workspace above.');
    } catch { notice('Could not open Cloud. Check your connection and retry.', true); }
    finally { sourceBusy = false; render(); }
    return;
  }
  const settingsScope = keepSettings && ['workspace', 'settings', 'connection'].includes(view) ? view : null;
  sourceBusy = true; render();
  try {
    await navigation;
    const selection = preference(id);
    if (selection.kind === 's3' && !selection.url) {
      await invoke('hide_dashboard');
      workspace = id; sourceKind = 's3'; lastDashboard = 'dashboard';
      persistWorkspace(); view = 'connection';
      sourceBusy = false;
      await refreshSource(); render();
    } else {
      await openPreference(id, selection);
      if (settingsScope) { sourceBusy = false; await navigate(settingsScope); }
    }
  } catch (error) {
    await invoke('hide_dashboard').catch(() => {});
    view = 'connection';
    notice(error instanceof Error ? error.message : String(error), true);
  } finally { sourceBusy = false; render(); }
}

const knownSetup = () => Boolean(browseOnly || state?.has_s3_reader || state?.service_installed || state?.has_history || state?.running);
const updateInstalling = () => updateAction === 'install_update' || ['downloading', 'installing', 'restarting'].includes(updates.phase);
const collectionBlocked = () => Boolean(collectionAction) || collectionSwitch || updateInstalling();
function notice(text, error = false) { message = text ? { text, error } : null; render(); }
function activityText(timestamp) {
  if (!timestamp) return 'Waiting for agent activity';
  const minutes = Math.max(0, Math.floor((Date.now() - timestamp) / 60000));
  if (minutes < 1) return 'Last activity just now';
  if (minutes < 60) return `Last activity ${minutes} min ago`;
  return `Last activity ${new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }).format(new Date(timestamp))}`;
}
function renderWorkspacePages() {
  const shown = ['workspace', 'connection'].includes(view);
  $('workspace-context').hidden = !shown;
  if (!shown) return;
  const pages = [['overview', 'Overview'], ['models', 'Models'], ['tools', 'Tools'], ['mcp', 'MCP'], ['skills', 'Skills'], ['patterns', 'Struggles'], ['sessions', 'Sessions'], ['subagents', 'Subagents']];
  if (sourceKind === 'cloud') pages.push(['team', 'Team'], ['members', 'Members'], ['devices', 'Devices']);
  pages.push(['workspace', 'Workspace Settings']);
  const nav = $('workspace-pages');
  const signature = JSON.stringify(pages);
  if (nav.dataset.pages !== signature) {
    nav.replaceChildren(...pages.map(([page, label]) => {
      const button = document.createElement('button');
      button.textContent = label;
      button.dataset.page = page;
      if (page === 'workspace') button.setAttribute('aria-current', 'page');
      button.addEventListener('click', () => void (page === 'workspace' ? navigate('workspace') : navigate(lastDashboard, page)));
      return button;
    }));
    nav.dataset.pages = signature;
  }
  for (const button of nav.children) button.disabled = sourceBusy || collectionSwitch;
}
function render() {
  renderWorkspacePages();
  for (const name of ['loading', 'onboarding', 'dashboard', 'settings', 'connection', 'workspace']) $(`${name}-view`).hidden = (view === 'cloud' ? 'dashboard' : view) !== name;
  $('navigation').hidden = !knownSetup();
  for (const name of ['dashboard', 'settings']) {
    if ((['cloud', 'workspace', 'connection'].includes(view) ? 'dashboard' : view) === name) $(`nav-${name}`).setAttribute('aria-current', 'page');
    else $(`nav-${name}`).removeAttribute('aria-current');
  }
  renderWorkspace();
  $('source-label').textContent = view === 'cloud' && sourceKind !== 'cloud' ? 'Cloud sign-in' : { local: 'This Mac', s3: 'S3', cloud: 'Kikimimi Cloud' }[sourceKind];
  let status = 'Checking collection…', detail = '', action = '';
  if (state) {
    if (state.running) { status = 'Collecting'; detail = activityText(state.last_event_ts); }
    else if (state.service_installed) { status = 'Collection stopped'; detail = 'Your saved history is still available.'; action = 'Resume'; }
    else { status = knownSetup() ? 'Collection is off' : 'Not set up yet'; detail = knownSetup() ? 'Your saved history is still available.' : 'Connect your agents to start collecting.'; action = view === 'onboarding' ? '' : 'Set up collection'; }
  }
  if (collectionAction) { status = collectionAction === 'enable' ? 'Setting up collection…' : collectionAction === 'resume' ? 'Resuming collection…' : 'Disconnecting…'; detail = 'This may take a few seconds.'; }
  if (statusError) { status = 'Unable to check collection'; detail = 'Your saved history is still available.'; action = 'Retry'; }
  if (message && ['dashboard', 'cloud'].includes(view)) detail = message.text;
  $('status').textContent = status;
  $('status-detail').textContent = detail;
  $('status-dismiss').hidden = !message || !['dashboard', 'cloud'].includes(view);
  $('status-dot').className = `status-dot ${state?.running && !statusError ? 'running' : 'stopped'}`;
  $('status-action').hidden = !action;
  $('status-action').textContent = action;
  $('status-action').disabled = collectionBlocked();
  $('enable').disabled = !$('consent').checked || collectionBlocked();
  $('enable').textContent = collectionAction === 'enable' ? 'Setting up collection…' : 'Start collecting';
  $('consent').disabled = collectionBlocked();
  $('back-to-history').hidden = !knownSetup();
  $('connection-badge').textContent = state?.running ? 'Collecting' : state?.service_installed ? 'Stopped' : 'Not connected';
  $('connection-badge').className = `badge ${state?.running ? 'running' : ''}`;
  $('login-detail').textContent = state?.service_installed ? 'Collection starts automatically when you log in.' : 'Enabled when you complete collection setup.';
  $('login-status').textContent = state?.service_installed ? 'On' : 'Off';
  $('connection-detail').textContent = state?.service_installed || state?.running ? 'Disconnecting removes agent settings. Your history is kept.' : 'Connect your agents to collect new activity.';
  $('disconnect').hidden = !state?.service_installed && !state?.running;
  $('disconnect').disabled = collectionBlocked();
  $('setup').hidden = Boolean(state?.service_installed || state?.running);
  $('setup').disabled = collectionBlocked();
  $('confirm-disconnect').disabled = collectionBlocked();
  const updateBusy = Boolean(updateAction) || ['checking', 'downloading', 'installing', 'restarting'].includes(updates.phase);
  $('check-updates').disabled = !updates.configured || updateBusy || Boolean(collectionAction);
  $('check-updates').textContent = updates.phase === 'checking' || updateAction === 'check_updates' ? 'Checking…' : 'Check for updates';
  $('auto-update').checked = Boolean(updates.auto_update);
  $('auto-update').disabled = !updates.configured || updateBusy || Boolean(collectionAction);
  $('install-update').disabled = !updates.configured || !updates.available_version || updateBusy || Boolean(collectionAction);
  $('version').textContent = updates.current_version ? `Version ${updates.current_version}` : '';
  const progress = { checking: 'Checking for updates…', downloading: 'Downloading update…', installing: 'Installing update…', restarting: 'Restarting kikimimi…', current: 'You’re up to date.' };
  $('update-status').textContent = !updates.configured ? 'Automatic updates are unavailable in this build.'
    : updates.error ? (updates.error_kind === 'install' ? 'The update could not be installed. You can try again.' : 'Could not check for updates. You can try again later.')
    : progress[updates.phase] || (updates.available_version ? 'A new version is available.' : 'Updates are checked in the background.');
  $('available-update').hidden = !updates.configured || !updates.available_version;
  $('update-description').textContent = updates.available_version ? `Version ${updates.available_version} is ready to install.` : '';
  $('update-error-details').hidden = !updates.error;
  $('update-error').textContent = updates.error || '';
  $('message').hidden = !message || ['dashboard', 'cloud'].includes(view);
  $('message').classList.toggle('error', Boolean(message?.error));
  $('message-text').textContent = message?.text || '';
  renderCollection();
}
async function refreshStatus() {
  if (statusRequest) return statusRequest;
  statusRequest = (async () => {
    try { state = await invoke('status'); statusError = false; }
    catch { statusError = true; }
    finally { statusRequest = null; render(); }
  })();
  return statusRequest;
}
async function refreshUpdates() {
  if (updateRequest) return updateRequest;
  updateRequest = (async () => {
    try { updates = { ...await invoke('update_status'), error_kind: updates.error_kind }; }
    catch { updates.error = 'Update status is unavailable.'; }
    finally { updateRequest = null; render(); }
  })();
  return updateRequest;
}
function navigate(next, page = null) {
  if (sourceBusy || collectionSwitch) return navigation;
  if (['dashboard', 'cloud'].includes(next) && sourceKind === 's3' && !preference(workspace).url) next = 'connection';
  if (sourceRecovery && ['dashboard', 'cloud'].includes(next)) return changeWorkspace(workspace, false);
  if (next === 'cloud') browseOnly = true;
  if (['dashboard', 'cloud'].includes(next)) { lastDashboard = next; localStorage.setItem('dashboard-source', next); }
  if (next === 'dashboard' && !knownSetup()) next = 'onboarding';
  if (next === 'onboarding') $('consent').checked = false;
  view = next;
  const id = ++navigationId;
  render();
  navigation = navigation.catch(() => {}).then(async () => {
    if (id !== navigationId) return;
    try {
      if (['dashboard', 'cloud'].includes(next)) {
        $('dashboard-title').textContent = 'Opening your dashboard…';
        $('viewer-detail').textContent = 'Your saved history is available even when collection is off.';
        $('viewer-spinner').hidden = false;
        $('retry-dashboard').hidden = true;
        await invoke(next === 'cloud' ? 'show_cloud' : 'show_dashboard', page ? { page } : undefined);
      } else { await invoke('hide_dashboard'); if (next === 'connection') await refreshSource();
        if (next === 'settings') await refreshStorage();
        if (next === 'workspace') { renderWorkspaceDetails(); void refreshCloud().then(renderWorkspaceDetails).catch(() => {}); } }
    } catch {
      if (id === navigationId) {
        if (!['dashboard', 'cloud'].includes(next)) {
          view = 'dashboard';
          notice('Could not open settings. Try again.', true);
          return;
        }
        $('dashboard-title').textContent = 'Couldn’t open your dashboard';
        $('viewer-detail').textContent = next === 'cloud' ? 'Check your internet connection and try again.' : 'Your history is safe. Try again to restart the history viewer.';
        $('viewer-spinner').hidden = true;
        $('retry-dashboard').hidden = false;
      }
    }
  });
  return navigation;
}
async function collection(command) {
  if (collectionBlocked()) return;
  collectionAction = command;
  notice(null);
  try {
    await invoke(command);
    await refreshStatus();
    if (command === 'enable') {
      $('consent').checked = false;
      notice('Collection is on. Restart existing agent sessions to load the new settings.');
      await navigate(lastDashboard);
    } else if (command === 'disconnect') notice('Disconnected. Your saved history is still available.');
    else notice('Collection resumed.');
  } catch {
    const errors = {
      enable: 'Could not finish setup. Try Start collecting again.',
      resume: 'Collection did not start. Try Resume, or check your connection in Settings.',
      disconnect: 'Could not disconnect completely. Try again in Settings.',
    };
    notice(errors[command], true);
  } finally {
    collectionAction = null;
    await refreshStatus();
    render();
  }
}
async function update(command, args) {
  if (updateAction || collectionBlocked() || updates.phase === 'checking') return;
  updateAction = command;
  updates.error_kind = command === 'install_update' ? 'install' : 'check';
  render();
  try { await invoke(command, args); }
  catch {
    if (command === 'set_auto_update') notice('Could not save your update preference. Try again.', true);
    // Check/install errors have persistent details under App updates.
  }
  finally { updateAction = null; await refreshUpdates(); }
}
function sourceFields() {
  for (const option of $('data-source').options) {
    option.disabled = workspace === 's3-team' ? option.value !== 's3' : isCloudTeam(workspace) ? option.value !== 'cloud' : false;
  }
  $('s3-fields').hidden = $('data-source').value !== 's3';
  $('source-url').required = $('data-source').value === 's3';
  $('cloud-source-help').hidden = $('data-source').value !== 'cloud';
}
async function refreshSource() {
  if (sourceBusy) return;
  try {
    const info = await invoke('read_source');
    const saved = preference(workspace);
    // Import the legacy reader connection into Personal once only.
    if (workspace === 'personal' && !saved.url && info.connection && !localStorage.getItem('workspaces-v1')) {
      Object.assign(saved, info.connection);
      preferences.personal = saved;
    }
    $('data-source').value = saved.kind;
    $('source-url').value = saved.url || '';
    $('source-profile').value = saved.profile || '';
    $('source-endpoint').value = saved.endpoint_url || '';
    $('source-error').hidden = true;
  } catch {
    $('source-error').textContent = 'Could not read saved source settings. Reopen Viewing Connection to retry.';
    $('source-error').hidden = false;
  }
  sourceFields(); render();
}
function sameS3(left, right) {
  return left && right && left.url?.replace(/\/+$/, '') === right.url?.replace(/\/+$/, '') && (left.endpoint_url || '') === (right.endpoint_url || '');
}
function targetWorkspaces(target) {
  if (!target) return [];
  const ids = [];
  if (target.cloud) ids.push(target.cloud.hosted ? target.cloud.org_kind === 'personal' ? 'personal' : `cloud:${target.cloud.org_slug}` : 'external-cloud');
  if (target.s3) ids.push(sameS3(target.s3, preferences['s3-team']) ? 's3-team' : sameS3(target.s3, preferences.personal) ? 'personal' : 'external-s3');
  return ids.length ? ids : ['personal'];
}
function appliedTarget() {
  if (!state?.running || statusError) return null;
  if (state.applied_collection_target) return state.applied_collection_target;
  return null;
}
function collectingWorkspaces() { return targetWorkspaces(appliedTarget()); }
function targetName(target) {
  if (!target) return 'Unknown destination';
  const names = [];
  if (target.cloud) names.push(target.cloud.org_kind === 'personal' ? 'Personal (Cloud)' : `${target.cloud.org_slug || 'Unknown workspace'} (Cloud)`);
  if (target.s3) names.push(`S3 · ${target.s3.url}`);
  return names.join(' + ') || 'Personal (This Mac only)';
}
function collectionChoices() {
  const choices = [{ id:'local', label:'Personal · This Mac only', kind:'local' }];
  for (const org of cloudOrgs) choices.push({id:`cloud:${org.slug}`,label:`${org.kind === 'personal' ? 'Personal' : org.name} · Cloud`,kind:'cloud',slug:org.slug,team:org.kind === 'team'});
  for (const id of ['personal','s3-team']) if (preferences[id]?.url) choices.push({id:`s3:${id}`,label:`${workspaceName(id)} · ${preferences[id].url}`,kind:'s3',s3:{url:preferences[id].url,profile:preferences[id].profile || null,endpoint_url:preferences[id].endpoint_url || null}});
  return choices;
}
function renderCollection() {
  const applied = appliedTarget();
  const configured = state?.collection_target;
  const pending = state?.running && configured && (!applied || JSON.stringify(configured) !== JSON.stringify(applied));
  const other = Boolean((applied || configured) && !targetWorkspaces(applied || configured).includes(workspace));
  const name = targetName(applied || configured);
  $('collection-current').textContent = !configured ? 'Collection destination is unknown. Refresh status before changing it.' : state.running ? pending ? `Waiting for collector confirmation · ${targetName(configured)}` : `Collecting on this Mac · ${name}` : `Collection stopped · ${name}`;
  if (state && !statusError && !collectionAction) {
    $('status').textContent = collectionSwitch ? 'Changing collection…' : state.running ? pending ? 'Collection destination pending' : `Collecting · ${name}` : `Collection stopped · ${name}`;
    if (other) $('status-detail').textContent = `You are viewing ${workspaceName(workspace)}.`;
    if (pending) $('status-detail').textContent = 'The running collector has not confirmed this destination yet.';
  }
  $('collection-bar').classList.toggle('collection-elsewhere', other || Boolean(pending));
  $('collection-change').hidden = !state || (view === 'settings') || (!other && !pending && configured);
  $('collection-change').disabled = collectionBlocked() || sourceBusy;
  $('review-collection').disabled = !configured || collectionBlocked() || sourceBusy;
  const choices = collectionChoices();
  const control = $('collection-destination');
  const signature = JSON.stringify(choices);
  if (control.dataset.choices !== signature && !collectionReview) {
    const old = control.value;
    control.replaceChildren(...choices.map(choice => new Option(choice.label, choice.id)));
    control.value = choices.some(choice => choice.id === old) ? old : choices.find(choice => choice.kind === 'cloud' && choice.slug === configured?.cloud?.org_slug || choice.kind === 's3' && sameS3(choice.s3,configured?.s3))?.id || 'local';
    control.dataset.choices = signature;
  }
  control.disabled = collectionBlocked();
}
$('collection-change').addEventListener('click', async () => {
  await navigate('settings');
  const choices = collectionChoices();
  const preferred = isCloudTeam(workspace) ? `cloud:${workspace.slice(6)}` : sourceKind === 'cloud' ? choices.find(c => c.kind === 'cloud' && !c.team)?.id : sourceKind === 's3' ? `s3:${workspace}` : 'local';
  if (choices.some(c => c.id === preferred)) $('collection-destination').value = preferred;
  $('collection-destination').focus();
});
$('review-collection').addEventListener('click', () => {
  if (collectionBlocked() || !state?.collection_target) return;
  const choice = collectionChoices().find(c => c.id === $('collection-destination').value);
  if (!choice) return;
  collectionReview = { ...choice, expected: structuredClone(state.collection_target) };
  $('collection-review').textContent = `${targetName(collectionReview.expected)} → ${choice.label}`;
  $('collection-sharing').textContent = choice.kind === 'local' ? 'This stops Cloud and S3 uploads from this Mac. Collection stays local.' : choice.kind === 's3' ? 'All repositories and all locally recorded fields will be exported to this bucket. This replaces any existing Cloud or S3 upload destination.' : choice.team ? 'Only repositories matching the patterns below will be shared with this team. This replaces any existing Cloud or S3 upload destination.' : 'Activity from this Mac will be sent to your Personal Cloud workspace. This replaces any existing Cloud or S3 upload destination.';
  $('collection-repositories').hidden = !choice.team;
  $('collection-repos').value = '';
  $('collection-all-repos').checked = false;
  $('collection-consent').checked = false;
  $('collection-dialog-error').hidden = true;
  $('collection-dialog').showModal();
});
$('cancel-collection').addEventListener('click', () => { if (!collectionSwitch) { $('collection-dialog').close(); collectionReview = null; } });
$('collection-dialog').addEventListener('cancel', event => { if (collectionSwitch) event.preventDefault(); else collectionReview = null; });
$('collection-switch-form').addEventListener('submit', async event => {
  event.preventDefault();
  if (collectionSwitch || !collectionReview || !$('collection-consent').checked) return;
  const repos = collectionReview.team ? $('collection-all-repos').checked ? ['*'] : $('collection-repos').value.split(/\n/).map(s => s.trim()).filter(Boolean) : [];
  if (collectionReview.team && !repos.length) { $('collection-dialog-error').textContent = 'Choose the repositories to share, or explicitly select all repositories.'; $('collection-dialog-error').hidden = false; return; }
  collectionSwitch = true;
  for (const el of $('collection-switch-form').elements) el.disabled = true;
  render();
  try {
    const { kind, expected, slug, s3 } = collectionReview;
    await invoke('switch_collection', { selection:{kind,expected,slug,s3,repo_patterns:repos} });
    $('collection-dialog').close(); collectionReview = null;
    await refreshStatus(); await refreshStorage();
    notice(state?.running ? 'Collection destination saved. Waiting for the collector to confirm the change.' : 'Collection destination saved. Resume collection when ready.');
  } catch (error) {
    $('collection-dialog-error').textContent = typeof error === 'string' ? error : 'Could not change collection. Refresh the destination and try again.';
    $('collection-dialog-error').hidden = false;
    await refreshStatus();
  } finally {
    collectionSwitch = false;
    for (const el of $('collection-switch-form').elements) el.disabled = false;
    render();
  }
});

function renderWorkspaceDetails() {
  const org = cloudOrgs.find(org => workspace === 'personal' ? org.kind === 'personal' : `cloud:${org.slug}` === workspace);
  const team = isCloudTeam(workspace);
  $('workspace-name').textContent = workspaceName(workspace);
  $('workspace-scope').textContent = team ? 'Cloud workspace · shared with its members' : workspace === 's3-team' ? 'Shared S3 · access managed in AWS' : 'Personal workspace';
  $('workspace-description').textContent = team ? 'Activity shared with this team in Kikimimi Cloud.' : workspace === 's3-team' ? 'A shared S3 export. Each member connects using their own AWS access.' : 'Your own activity from this Mac, Kikimimi Cloud, or an S3 export.';
  $('workspace-access').textContent = team ? org ? `Your role: ${org.role}. Member and invitation controls depend on this role.` : 'Connect to Cloud to verify your membership and permissions.' : workspace === 's3-team' ? 'The bucket owner manages access in AWS.' : 'Choose a team from the Workspace menu to view shared activity.';
  $('manage-team').hidden = !team || !org;
  $('manage-team').textContent = ['owner', 'admin'].includes(org?.role) ? 'Manage members & invitations' : 'View team members';
  $('workspace-connection').textContent = sourceKind === 's3' ? preference(workspace).url || 'No bucket connected' : sourceKind === 'cloud' ? 'Kikimimi Cloud activity' : 'Activity saved on this Mac';
}
let storageRequest = null;
async function refreshStorage() {
  if (storageRequest) return storageRequest;
  $('upload-status').textContent = 'Reading this Mac’s destinations…';
  $('upload-destinations').hidden = true;
  storageRequest = (async () => {
    try {
      const settings = await invoke('read_storage');
      const rows = [['Local history', settings.local_path], ['Cloud uploads', settings.cloud ? `Configured · ${settings.cloud.org_slug || 'workspace not recorded'}` : 'Not configured'], ['S3 uploads', settings.s3 ? `Configured · ${settings.s3.url}` : 'Not configured']];
      if (settings.cloud?.org_kind === 'team') rows.push(['Shared repositories', settings.cloud.repo_patterns.length ? settings.cloud.repo_patterns.join(', ') : 'All repositories (no filter configured)']);
      $('upload-destinations').replaceChildren(...rows.flatMap(([label, value]) => {
        const term = document.createElement('dt'); term.textContent = label;
        const detail = document.createElement('dd'); detail.textContent = value;
        return [term, detail];
      }));
      $('upload-destinations').hidden = false;
      $('upload-status').textContent = 'Configured destinations for this Mac';
    } catch { $('upload-status').textContent = 'Could not read destinations. Upload configuration is unknown. Retry with Refresh destinations.'; }
    finally { storageRequest = null; }
  })();
  return storageRequest;
}
for (const [id, destination] of [['workspace-connection-settings', 'connection'], ['connection-workspace', 'workspace'], ['connection-app', 'settings'], ['workspace-app', 'settings']]) {
  $(id).addEventListener('click', (event) => { event.preventDefault(); void navigate(destination); });
}
$('refresh-storage').addEventListener('click', () => void refreshStorage());
$('manage-team').addEventListener('click', async () => {
  if (sourceBusy || !isCloudTeam(workspace)) return;
  sourceBusy = true; render();
  try {
    await invoke('select_cloud_workspace', { slug: workspace.slice('cloud:'.length) });
    await invoke('show_cloud', { page: 'team' });
    view = 'cloud';
  } catch { notice('Could not open team settings. Check your connection and try again.', true); }
  finally { sourceBusy = false; render(); }
});
$('workspace').addEventListener('change', event => void changeWorkspace(event.target.value));
$('data-source').addEventListener('change', sourceFields);
$('source-form').addEventListener('submit', async event => {
  event.preventDefault();
  if (sourceBusy) return;
  sourceBusy = true;
  const kind = $('data-source').value;
  const selection = kind === 's3' ? { kind, url: $('source-url').value.trim(), profile: $('source-profile').value.trim() || null, endpoint_url: $('source-endpoint').value.trim() || null } : { kind };
  for (const control of $('source-form').elements) control.disabled = true;
  $('source-error').hidden = true;
  $('apply-source').textContent = kind === 's3' ? 'Connecting to S3…' : 'Applying…';
  try {
    await navigation;
    if ((workspace === 's3-team' && kind !== 's3') || (isCloudTeam(workspace) && kind !== 'cloud')) throw new Error('Choose the data source for this workspace.');
    await openPreference(workspace, selection);
  } catch (error) {
    await invoke('hide_dashboard').catch(() => {});
    view = 'connection';
    $('source-error').textContent = error instanceof Error ? error.message : String(error);
    $('source-error').hidden = false;
  } finally {
    sourceBusy = false;
    for (const control of $('source-form').elements) control.disabled = false;
    $('apply-source').textContent = 'Connect and view';
    sourceFields(); render();
  }
});
$('consent').addEventListener('change', render);
$('browse-only').addEventListener('click', () => { browseOnly = true; void navigate('dashboard'); });
$('onboarding-cloud').addEventListener('click', () => { preferences.personal = { kind: 'cloud' }; void changeWorkspace('personal'); });
$('enable').addEventListener('click', () => { if ($('consent').checked) void collection('enable'); });
$('status-action').addEventListener('click', async () => {
  if (statusError) {
    await refreshStatus();
    if (view === 'loading' && !statusError) await navigate(lastDashboard === 'cloud' ? 'cloud' : knownSetup() ? 'dashboard' : 'onboarding');
  } else if (state?.service_installed) void collection('resume');
  else void navigate('onboarding');
});
for (const [id, destination] of [['nav-dashboard', null], ['nav-settings', 'settings'], ['history', null], ['back-to-history', null], ['setup', 'onboarding'], ['retry-dashboard', null]]) {
  $(id).addEventListener('click', () => void navigate(destination || lastDashboard));
}
document.querySelector('.brand').addEventListener('click', (event) => { event.preventDefault(); void navigate(lastDashboard); });
$('auto-update').addEventListener('change', () => void update('set_auto_update', { enabled: $('auto-update').checked }));
$('check-updates').addEventListener('click', () => void update('check_updates'));
$('install-update').addEventListener('click', () => void update('install_update'));
$('disconnect').addEventListener('click', () => $('disconnect-dialog').showModal());
$('cancel-disconnect').addEventListener('click', () => $('disconnect-dialog').close());
$('confirm-disconnect').addEventListener('click', () => { $('disconnect-dialog').close(); void collection('disconnect'); });
$('dismiss-message').addEventListener('click', () => notice(null));
$('status-dismiss').addEventListener('click', () => notice(null));
window.__TAURI__.event?.listen('navigate', ({ payload }) => {
  if (['dashboard', 'cloud', 'settings', 'connection', 'workspace'].includes(payload)) void navigate(payload === 'dashboard' ? lastDashboard : payload);
});
window.addEventListener('keydown', (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key === ',') { event.preventDefault(); void navigate('settings'); }
});
async function start() {
  render();
  await refreshStatus();
  await refreshSource();
  if (sourceKind === 'cloud') {
    sourceBusy = true; render();
    try { await openPreference(workspace, preference(workspace)); }
    catch { sourceBusy = false; await navigate('connection'); notice('Could not reopen Cloud. Check your sign-in and retry.', true); }
    finally { sourceBusy = false; render(); }
  } else void refreshCloud().catch(() => {});
  if (sourceKind === 's3' && preference(workspace).url) {
    try { await invoke('set_source', { selection: preference(workspace) }); }
    catch { sourceRecovery = true; await navigate('connection'); notice('Could not reopen S3. Check the connection and retry.', true); }
  } else if (sourceKind === 's3') { await navigate('connection'); }
  else if (sourceKind === 'local' && knownSetup()) {
    try { await invoke('set_source', { selection: { kind: 'local' } }); }
    catch { sourceRecovery = true; await navigate('connection'); notice('Could not reopen local history. Try applying This Mac again.', true); }
  }
  if (statusError) $('loading-view').querySelector('p').textContent = 'Could not check your setup. Use Retry above.';
  else if (view === 'loading') await navigate(lastDashboard === 'cloud' ? 'cloud' : knownSetup() ? 'dashboard' : 'onboarding');
  await refreshUpdates();
}
void start();
setInterval(() => { if (!collectionAction) void refreshStatus(); }, 5000);
setInterval(() => void refreshUpdates(), 2000);

setInterval(() => { if (!sourceBusy && view === 'cloud') void refreshCloud(signingIn).catch(() => {}); }, 10000);

$('workspace-signin').addEventListener('click', () => void changeWorkspace('signin'));
