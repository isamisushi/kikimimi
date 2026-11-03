import { t, intlLocale } from "../../web/src/i18n";
import { configureTransport } from "../../web/src/api/client";
import {
  Browsing,
  Connections,
  CollectionDestination,
  automaticWorkspace,
} from "./workspace-domain";
import type {
  View,
  Source,
  Connection,
  Status,
  Updates,
  RouterStatus,
  OpenCodeRouterStatus,
  Storage,
  Choice,
  CollectionChange,
  Destination,
  Invoke,
} from "./types";

const parse = <T>(key: string, fallback: T): T => {
  try {
    return JSON.parse(localStorage.getItem(key) || "null") || fallback;
  } catch {
    return fallback;
  }
};
const errorText = (e: unknown, fallback: string) =>
  typeof e === "string" ? e : e instanceof Error ? e.message : fallback;
export function targetName(target?: Destination | null) {
  if (!target) return t("Unknown destination");
  return (
    [
      target.cloud && target.cloud.sync_enabled !== false &&
        (target.cloud.org_kind === "personal"
          ? t("Personal (Cloud)")
          : `${target.cloud.org_slug} (Cloud)`),
      target.s3 && `S3 · ${target.s3.url}`,
    ]
      .filter(Boolean)
      .join(" + ") || t("This Mac")
  );
}
export function activityText(timestamp?: number) {
  if (!timestamp) return t("Waiting for agent activity");
  const minutes = Math.max(0, Math.floor((Date.now() - timestamp) / 60000));
  return minutes < 1
    ? t("Last activity just now")
    : minutes < 60
      ? t("Last activity {0} min ago", [minutes])
      : t("Last activity {0}", [
          new Date(timestamp).toLocaleString(intlLocale()),
        ]);
}
// Application services own effects. React owns all DOM, form state, and rendering.
export class DesktopController {
  invoke: Invoke;
  listeners = new Set<() => void>();
  revision = 0;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  snapshot = () => this.revision;
  emit = () => {
    this.revision++;
    this.listeners.forEach((fn) => fn());
  };
  status: Status | null = null;
  statusError = false;
  updates: Updates = { configured: false, phase: "idle" };
  view: View = "loading";
  reader: Source = "local";
  workspace = "personal";
  preferences: Record<string, Connection> = {};
  cloudSlug = "";
  cloudWorkspaces: {
    slug: string;
    name?: string;
    kind: "personal" | "team";
  }[] = [];
  workspacesBusy = false;
  workspacesError = "";
  uploadHistory = false;
  cloudKind: "personal" | "team" = "team";
  browseOnly = false;
  message: { text: string; error: boolean } | null = null;
  sourceBusy = false;
  collectionAction: string | null = null;
  collectionSwitch = false;
  updateAction: string | null = null;
  settingsPage: "collection" | "codex" | "opencode" | "general" = "collection";
  storage: Storage | null = null;
  storageError = "";
  storageBusy = false;
  openCodeRouter: OpenCodeRouterStatus | null = null;
  openCodeRouterTeam = "";
  openCodeRouterBusy = false;
  openCodeRouterError = "";
  router: RouterStatus | null = null;
  routerTeam = "";
  routerBusy = false;
  routerError = "";
  consent = false;
  disconnectOpen = false;
  collectionReview: CollectionChange | null = null;
  collectionConsent = false;
  collectionError = "";
  destination = "local";
  sourceForm: Connection = { kind: "local" };
  sourceError = "";
  viewerError = false;
  viewerTitle = "Opening your dashboard…";
  viewerDetail = "Loading your saved history.";
  browsing: Browsing;
  connections: Connections;
  collectionDestination: CollectionDestination;
  navigation = Promise.resolve();
  statusRequest: Promise<void> | null = null;
  updatesRequest: Promise<void> | null = null;
  historySource: "local" | "s3" = "local";
  historyReady = false;
  historyKey = 0;
  disposed = false;
  constructor(invoke: Invoke = window.__TAURI__.core.invoke) {
    this.invoke = invoke;
    configureTransport(async (path, init) => {
      const result = await invoke<{ status: number; body: string }>(
        "history_request",
        {
          path,
          method: init?.method || "GET",
          source: this.historySource,
          body: init?.body ? JSON.parse(String(init.body)) : null,
        },
      );
      return new Response(result.status === 204 ? null : result.body, {
        status: result.status,
        headers: { "Content-Type": "application/json" },
      });
    });
    const legacy = parse<{
      active?: string;
      preferences?: Record<
        string,
        Omit<Connection, "kind"> & { kind: Source | "cloud" }
      >;
    }>("workspaces-v1", {});
    this.workspace = legacy.active || "personal";
    this.preferences = Object.fromEntries(
      Object.entries(legacy.preferences || {})
        .filter(([id]) => id === "personal" || id === "s3-team")
        .map(([id, connection]) => [
          id,
          { ...connection, kind: connection.kind === "s3" ? "s3" : "local" },
        ]),
    );
    if (!this.preferences.personal)
      this.preferences.personal = {
        kind: localStorage.getItem("data-source") === "s3" ? "s3" : "local",
      };
    const saved = parse<{
      destination?: View | "cloud";
      reader?: Source;
      workspace?: string;
    }>("browsing-v1", {});
    this.reader =
      saved.destination !== "cloud" &&
      (saved.reader || this.preference().kind) === "s3"
        ? "s3"
        : "local";
    this.workspace =
      this.reader === "s3" && saved.workspace === "s3-team"
        ? "s3-team"
        : "personal";
    this.browsing = new Browsing(
      {
        history: async (source) => {
          if (this.historySource !== source) this.historyKey++;
          this.historySource = source as "local" | "s3";
          this.historyReady = true;
          this.emit();
        },
      },
      { destination: "dashboard", reader: this.reader },
    );
    this.connections = new Connections(
      {
        s3: (selection) => invoke("set_source", { selection }),
      },
      this.preferences,
    );
    this.preferences = this.connections.saved;
    this.collectionDestination = new CollectionDestination((selection) =>
      invoke("switch_collection", { selection }),
    );
  }
  preference(id = this.workspace): Connection {
    return id === "s3-team"
      ? { ...this.preferences[id], kind: "s3" }
      : this.preferences[id] || { kind: "local" };
  }
  workspaceName(id = this.workspace) {
    return id === "personal"
      ? "Personal"
      : id === "s3-team"
        ? "Team · S3"
        : "Personal";
  }
  get knownSetup() {
    return Boolean(
      this.browseOnly ||
      this.status?.has_history ||
      this.status?.service_installed ||
      this.status?.has_s3_reader ||
      this.status?.running,
    );
  }
  get installing() {
    return (
      this.updateAction === "install_update" ||
      ["downloading", "installing", "restarting"].includes(this.updates.phase)
    );
  }
  get collectionBlocked() {
    return Boolean(
      this.collectionAction || this.collectionSwitch || this.installing,
    );
  }
  get updateBusy() {
    return (
      Boolean(this.updateAction) ||
      ["checking", "downloading", "installing", "restarting"].includes(
        this.updates.phase,
      )
    );
  }
  get pending() {
    const s = this.status;
    return Boolean(
      s?.running &&
      s.collection_target &&
      (!s.applied_collection_target ||
        JSON.stringify(s.collection_target) !==
          JSON.stringify(s.applied_collection_target)),
    );
  }
  get applied() {
    return this.status?.running && !this.statusError
      ? this.status.applied_collection_target
      : null;
  }
  get target() {
    return this.applied || this.status?.collection_target;
  }
  get collectionLabel() {
    const s = this.status;
    return this.statusError
      ? t("Unable to check collection")
      : this.collectionSwitch
        ? t("Changing collection…")
        : this.collectionAction === "enable"
          ? t("Setting up collection…")
          : this.collectionAction === "resume"
            ? t("Resuming collection…")
            : this.collectionAction === "disconnect"
              ? t("Disconnecting…")
              : this.pending
                ? t("Applying collection and sync settings…")
                : !s
                  ? t("Checking collection…")
                  : `${s.running ? t("Collecting on this Mac") : !s.service_installed && !s.has_history ? t("Not set up") : t("Collection stopped")} · ${t(this.cloudSyncEnabled ? "Cloud sync on" : "Cloud sync off")}`;
  }
  get statusAction() {
    return this.statusError
      ? t("Retry")
      : this.status?.running
        ? ""
        : this.status?.service_installed
          ? t("Resume")
          : this.view === "onboarding"
            ? ""
            : t("Set up collection");
  }
  notice(text: string | null, error = false) {
    this.message = text ? { text, error } : null;
    this.emit();
  }
  persistBrowsing() {
    localStorage.setItem(
      "browsing-v1",
      JSON.stringify({
        destination: "dashboard",
        reader: this.browsing.reader,
        workspace: this.workspace,
      }),
    );
  }
  persistConnections() {
    localStorage.setItem(
      "workspaces-v1",
      JSON.stringify({ active: this.workspace, preferences: this.preferences }),
    );
  }
  async refreshStatus() {
    if (this.statusRequest) return this.statusRequest;
    this.statusRequest = (async () => {
      try {
        this.status = await this.invoke<Status>("status");
        this.statusError = false;
      } catch {
        this.statusError = true;
      } finally {
        this.statusRequest = null;
        this.emit();
      }
    })();
    return this.statusRequest;
  }
  async refreshUpdates() {
    if (this.updatesRequest) return this.updatesRequest;
    this.updatesRequest = (async () => {
      try {
        this.updates = {
          ...(await this.invoke<Updates>("update_status")),
          error_kind: this.updates.error_kind,
        };
      } catch {
        this.updates.error = "Update status is unavailable.";
      } finally {
        this.updatesRequest = null;
        this.emit();
      }
    })();
    return this.updatesRequest;
  }
  async refreshStorage() {
    if (this.storageBusy) return;
    this.storageBusy = true;
    this.storageError = "";
    this.emit();
    try {
      this.storage = await this.invoke<Storage>("read_storage");
    } catch {
      this.storageError =
        "Could not read destinations. Upload configuration is unknown. Retry with Refresh destinations.";
    } finally {
      this.storageBusy = false;
      this.emit();
    }
  }
  async refreshRouter() {
    try {
      this.router = await this.invoke<RouterStatus>("codex_router_status");
      this.routerError = "";
    } catch {
      this.router = null;
      this.routerError = "Could not read Codex settings.";
    }
    this.emit();
  }
  async refreshOpenCodeRouter() {
    try {
      this.openCodeRouter = await this.invoke<OpenCodeRouterStatus>(
        "opencode_router_status",
      );
      this.openCodeRouterError = "";
    } catch {
      this.openCodeRouter = null;
      this.openCodeRouterError = "Could not read OpenCode settings.";
    }
    this.emit();
  }
  async refreshSource() {
    try {
      const info = await this.invoke<{ connection?: Omit<Connection, "kind"> }>(
        "read_source",
      );
      this.sourceForm = { ...this.preference() };
      if (
        this.workspace === "personal" &&
        !this.sourceForm.url &&
        info.connection &&
        !localStorage.getItem("workspaces-v1")
      ) {
        Object.assign(this.sourceForm, info.connection);
        this.preferences.personal = { ...this.sourceForm };
      }
      this.sourceError = "";
    } catch {
      this.sourceError =
        "Could not read saved source settings. Reopen Viewing Connection to retry.";
    }
    this.emit();
  }
  navigate(next: View) {
    if (this.sourceBusy || this.collectionSwitch) return this.navigation;
    if (next === "dashboard" && this.reader === "s3" && !this.preference().url)
      next = "connection";
    if (next === "dashboard" && !this.knownSetup) next = "onboarding";
    if (next === "onboarding") this.consent = false;
    this.view = next;
    this.viewerError = false;
    this.viewerTitle = "Opening your dashboard…";
    this.viewerDetail = "Loading your saved history.";
    const revision = this.browsing.select(
      next,
      this.reader === "s3" ? "s3" : this.browsing.reader,
    );
    this.persistBrowsing();
    this.emit();
    this.navigation = this.navigation
      .catch(() => {})
      .then(async () => {
        if (!this.browsing.current(revision)) return;
        try {
          if (next === "dashboard") await this.browsing.present(revision);
          else if (next === "connection") await this.refreshSource();
          else if (next === "settings") {
            void this.refreshRouter();
            void this.refreshOpenCodeRouter();
            await this.refreshStorage();
          }
        } catch {
          if (this.browsing.current(revision)) {
            this.viewerError = true;
            this.viewerTitle = "Couldn’t open your dashboard";
            this.viewerDetail =
              "Your history is safe. Check the connection and try again.";
            this.emit();
          }
        }
      });
    return this.navigation;
  }
  async openLocal() {
    if (
      this.sourceBusy ||
      this.collectionSwitch ||
      (this.view === "dashboard" &&
        this.reader === "local" &&
        !this.viewerError &&
        !this.message?.error)
    )
      return;
    this.browseOnly = true;
    this.reader = "local";
    this.workspace = "personal";
    this.message = null;
    this.browsing.reader = "local";
    await this.navigate("dashboard");
  }
  async openCloud(page = "overview") {
    try {
      await this.invoke("show_cloud", { page });
    } catch (e) {
      this.notice(
        errorText(e, "Could not open your browser. Try again."),
        true,
      );
    }
  }
  async connectAndView(id: string, selection: Connection) {
    await this.connections.configure(id, selection);
    this.workspace = id;
    this.reader = selection.kind;
    this.view = "dashboard";
    this.browseOnly = true;
    this.message = null;
    this.persistConnections();
    const revision = this.browsing.select(
      this.view,
      selection.kind === "s3" ? "s3" : "local",
    );
    this.persistBrowsing();
    this.viewerError = false;
    this.emit();
    await this.browsing.present(revision);
  }
  async changeWorkspace(id: string) {
    if (this.sourceBusy || this.collectionSwitch) return;
    const scope = ["workspace", "settings", "connection"].includes(this.view)
      ? this.view
      : null;
    this.sourceBusy = true;
    this.emit();
    try {
      await this.navigation;
      const selection = this.preference(id);
      if (selection.kind === "s3" && !selection.url) {
        this.workspace = id;
        this.reader = "s3";
        this.view = "connection";
        this.sourceBusy = false;
        await this.refreshSource();
      } else {
        await this.connectAndView(id, selection);
        if (scope) {
          this.sourceBusy = false;
          await this.navigate(scope);
        }
      }
    } catch (e) {
      this.view = "connection";
      this.notice(errorText(e, "Could not change connection."), true);
    } finally {
      this.sourceBusy = false;
      this.emit();
    }
  }
  async applySource() {
    if (this.sourceBusy) return;
    this.sourceBusy = true;
    this.sourceError = "";
    this.emit();
    try {
      await this.navigation;
      await this.connectAndView(this.workspace, { ...this.sourceForm });
    } catch (e) {
      this.view = "connection";
      this.sourceError = errorText(e, "Could not connect.");
    } finally {
      this.sourceBusy = false;
      this.emit();
    }
  }
  async collection(command: "enable" | "resume" | "disconnect") {
    if (this.collectionBlocked || (command === "enable" && !this.consent))
      return;
    this.collectionAction = command;
    this.message = null;
    this.emit();
    const codexBefore = this.status?.codex_collection;
    try {
      await this.invoke(command);
      await this.refreshStatus();
      // Codex reads config.toml only at launch, so the new [otel] waits for a restart.
      const restartCodex =
        command !== "disconnect" &&
        codexBefore !== "otel" &&
        this.status?.codex_collection === "otel";
      if (command === "enable") {
        this.consent = false;
        this.notice(
          restartCodex
            ? "Collection is on. Restart existing agent sessions, including Codex, to load the new settings."
            : "Collection is on. Restart existing agent sessions to load the new settings.",
        );
        await this.navigate("dashboard");
      } else
        this.notice(
          command === "disconnect"
            ? "Disconnected. Your saved history is still available."
            : restartCodex
              ? "Collection resumed. Restart Codex to measure its usage in real time."
              : "Collection resumed.",
        );
    } catch {
      this.notice(
        "Could not change collection. Your saved history is safe. Try again.",
        true,
      );
    } finally {
      this.collectionAction = null;
      await this.refreshStatus();
      this.emit();
    }
  }
  async loadCloudWorkspaces() {
    if (this.workspacesBusy) return;
    this.workspacesBusy = true;
    this.workspacesError = "";
    this.emit();
    try {
      const result = await this.invoke<{
        orgs: { slug: string; name?: string; kind: "personal" | "team" }[];
        authorized_slug?: string;
      }>("collection_workspaces");
      this.cloudWorkspaces = result.orgs.filter(
        (o) => o.kind === "team" || o.kind === "personal",
      );
      const selected =
        this.cloudWorkspaces.find((o) => o.slug === this.cloudSlug) ??
        (this.cloudWorkspaces.length === 1
          ? this.cloudWorkspaces[0]
          : undefined);
      this.cloudSlug = selected?.slug ?? "";
      this.cloudKind = selected?.kind ?? "team";
      if (!this.cloudWorkspaces.length)
        this.workspacesError = "No workspaces available for this account.";
      return result.authorized_slug;
    } catch {
      this.workspacesError = "Could not load workspaces. Sign in and retry.";
    } finally {
      this.workspacesBusy = false;
      this.emit();
    }
  }
  async beginCloudConnection() {
    if (this.collectionBlocked || this.workspacesBusy) return false;
    this.destination = "cloud";
    const authorized = await this.loadCloudWorkspaces();
    if (this.workspacesError) return false;
    const slug = automaticWorkspace(this.cloudWorkspaces, authorized);
    if (!slug) { this.cloudSlug = ""; this.emit(); return false; }
    this.cloudSlug = slug;
    return this.connectCloudWorkspace();
  }
  async connectCloudWorkspace() {
    if (this.collectionBlocked || this.statusError || !this.status?.collection_target) return false;
    const org = this.cloudWorkspaces.find((o) => o.slug === this.cloudSlug);
    if (!org) return false;
    this.collectionSwitch = true;
    this.workspacesError = "";
    this.emit();
    try {
      const expected = structuredClone(this.status.collection_target);
      await this.collectionDestination.change(expected.cloud?.org_slug === org.slug
        ? { id: "cloud-sync", label: "Cloud sync", kind: "sync-on", expected }
        : { id: "cloud", label: org.name || org.slug, kind: "cloud", slug: org.slug,
            team: org.kind === "team", upload_history: true, expected });
      await this.refreshStatus();
      await this.refreshStorage();
      return true;
    } catch (e) {
      this.workspacesError = errorText(e, "Could not connect Cloud sync. Try again.");
      return false;
    } finally { this.collectionSwitch = false; this.emit(); }
  }
  get cloudSyncEnabled() {
    const cloud = this.status?.collection_target?.cloud;
    return Boolean(cloud && cloud.sync_enabled !== false);
  }
  get cloudSyncStatus() {
    if (!this.cloudSyncEnabled) return t("Off");
    if (this.pending) return t("Applying sync settings…");
    if (!this.status?.running) return t("Collection stopped");
    const sync = this.status?.cloud_sync;
    if (sync?.last_error) return t("Sync error");
    if (!sync || !sync.last_push_at) return t("Connecting…");
    return sync.pending > 0 ? `${t("Syncing")} · ${sync.pending}` : t("Synced");
  }
  async toggleCloudSync() {
    if (this.collectionBlocked || this.statusError || !this.status?.collection_target?.cloud) return;
    this.collectionSwitch = true;
    this.emit();
    try {
      await this.collectionDestination.change({
        id: "cloud-sync", label: "Cloud sync",
        kind: this.cloudSyncEnabled ? "sync-off" : "sync-on",
        expected: structuredClone(this.status.collection_target),
      });
      await this.refreshStatus();
    } catch (e) {
      this.notice(errorText(e, "Could not update Cloud sync."));
    } finally {
      this.collectionSwitch = false;
      this.emit();
    }
  }
  choices(): Choice[] {
    const choices: Choice[] = [
      { id: "local", label: t("This Mac only"), kind: "local" },
    ];
    choices.push({
      id: "cloud",
      label: t("Cloud · authorize in browser"),
      kind: "cloud",
      slug: this.cloudSlug.trim(),
      team: this.cloudKind === "team",
    });
    for (const id of ["personal", "s3-team"])
      if (this.preferences[id]?.url)
        choices.push({
          id: `s3:${id}`,
          label: `${this.workspaceName(id)} · ${this.preferences[id].url}`,
          kind: "s3",
          s3: {
            url: this.preferences[id].url,
            profile: this.preferences[id].profile || null,
            endpoint_url: this.preferences[id].endpoint_url || null,
          },
        });
    return choices;
  }
  reviewCollection() {
    if (
      this.statusError ||
      this.collectionBlocked ||
      !this.status?.collection_target
    )
      return;
    const choice = this.choices().find((c) => c.id === this.destination);
    if (!choice) return;
    if (choice.kind === "cloud" && !choice.slug) return;
    this.collectionReview = {
      ...choice,
      label:
        choice.kind === "cloud"
          ? `${choice.slug} (${this.cloudKind}) · Cloud`
          : choice.label,
      expected: structuredClone(this.status.collection_target),
    };
    this.uploadHistory = choice.kind === "cloud";
    this.collectionConsent = false;
    this.collectionError = "";
    this.emit();
  }
  async changeCollection() {
    if (
      this.collectionSwitch ||
      !this.collectionReview ||
      !this.collectionConsent
    )
      return;
    const review = this.collectionReview;
    this.collectionSwitch = true;
    this.emit();
    try {
      await this.collectionDestination.change({
        ...review,
        upload_history: this.uploadHistory,
      });
      this.collectionReview = null;
      if (this.uploadHistory)
        this.notice(
          "Saved history queued for synchronization. Local history remains on this Mac.",
        );
      await this.refreshStatus();
      await this.refreshStorage();
      if (!this.uploadHistory)
        this.notice(
          this.status?.running
            ? "Collection destination saved. Waiting for the collector to confirm the change."
            : "Collection destination saved. Resume collection when ready.",
        );
    } catch (e) {
      this.collectionError = errorText(
        e,
        "Could not change collection. Refresh the destination and try again.",
      );
      await this.refreshStatus();
    } finally {
      this.collectionSwitch = false;
      this.emit();
    }
  }
  async update(
    command: "check_updates" | "install_update" | "set_auto_update",
    args?: Record<string, unknown>,
  ) {
    if (
      this.updateAction ||
      this.collectionBlocked ||
      this.updates.phase === "checking"
    )
      return;
    this.updateAction = command;
    this.updates.error_kind =
      command === "install_update" ? "install" : "check";
    this.emit();
    try {
      await this.invoke(command, args);
    } catch {
      if (command === "set_auto_update")
        this.notice("Could not save your update preference. Try again.", true);
    } finally {
      this.updateAction = null;
      await this.refreshUpdates();
      this.emit();
    }
  }
  async connectRouter() {
    if (this.routerBusy || !this.routerTeam) return;
    this.routerBusy = true;
    this.routerError = "";
    this.emit();
    let error = "";
    try {
      await this.invoke("connect_codex_router", { slug: this.routerTeam });
    } catch (e) {
      error = errorText(
        e,
        "Could not authorize this Mac in your browser. Check your team name and retry.",
      );
    }
    await this.refreshRouter();
    this.routerBusy = false;
    this.routerError = error;
    if (!error)
      this.notice(
        "Team connected. Turn on routing when you’re ready. Collection settings are unchanged.",
      );
    this.emit();
  }
  async setRouter(enabled: boolean) {
    if (this.routerBusy) return;
    this.routerBusy = true;
    this.routerError = "";
    this.emit();
    let error = "";
    try {
      await this.invoke("set_codex_router", { enabled });
    } catch (e) {
      const detail = String(e);
      error = detail.includes("no active router")
        ? "This team has no router yet. Ask a team admin to set one up."
        : detail.includes("Applications")
          ? detail
          : "Could not change Codex settings. Check your connection and try again.";
    }
    await this.refreshRouter();
    this.routerBusy = false;
    this.routerError = error;
    if (!error)
      this.notice(
        enabled
          ? "Router settings saved. Open a new Codex chat to try the connection."
          : "Previous provider restored. Open a new Codex chat to use it.",
      );
    this.emit();
  }
  async connectOpenCodeRouter() {
    if (this.openCodeRouterBusy || !this.openCodeRouterTeam) return;
    this.openCodeRouterBusy = true;
    this.openCodeRouterError = "";
    this.emit();
    let error = "";
    try {
      await this.invoke("connect_opencode_router", {
        slug: this.openCodeRouterTeam,
      });
    } catch (e) {
      error = errorText(
        e,
        "Could not authorize this Mac in your browser. Check your team name and retry.",
      );
    }
    await this.refreshOpenCodeRouter();
    this.openCodeRouterBusy = false;
    this.openCodeRouterError = error;
    if (!error)
      this.notice(
        "Team connected. Turn on routing when you’re ready. Collection settings are unchanged.",
      );
    this.emit();
  }
  async setOpenCodeRouter(enabled: boolean) {
    if (this.openCodeRouterBusy) return;
    this.openCodeRouterBusy = true;
    this.openCodeRouterError = "";
    this.emit();
    let error = "";
    try {
      await this.invoke("set_opencode_router", { enabled });
    } catch (e) {
      const detail = String(e);
      error = detail.includes("no active router")
        ? "This team has no router yet. Ask a team admin to set one up."
        : detail.includes("Applications")
          ? detail
          : errorText(
              e,
              "Could not change OpenCode settings. Check your connection and try again.",
            );
    }
    await this.refreshOpenCodeRouter();
    this.openCodeRouterBusy = false;
    this.openCodeRouterError = error;
    if (!error)
      this.notice(
        enabled
          ? "Router settings saved. Restart OpenCode to try the connection."
          : "Previous provider restored. Restart OpenCode to use it.",
      );
    this.emit();
  }
  async statusActionClick() {
    if (this.statusError) {
      await this.refreshStatus();
      if (this.view === "loading" && !this.statusError)
        await this.navigate(this.knownSetup ? "dashboard" : "onboarding");
    } else if (this.status?.service_installed) await this.collection("resume");
    else await this.navigate("onboarding");
  }
  async start() {
    this.disposed = false;
    await this.refreshStatus();
    if (this.disposed) return;
    await this.refreshSource();
    if (this.disposed) return;
    if (this.reader === "s3" && !this.preference().url)
      await this.navigate("connection");
    else if (!this.statusError && this.view === "loading")
      await this.navigate(this.knownSetup ? "dashboard" : "onboarding");
    void this.refreshUpdates();
  }
  mount() {
    const cleanups: (() => void)[] = [];
    const listen = (name: string, fn: (e: { payload: string }) => void) => {
      const result = window.__TAURI__.event?.listen(name, fn);
      if (result)
        void result.then((off) => {
          if (this.disposed) off();
          else cleanups.push(off);
        });
    };
    listen("navigate", ({ payload }) => {
      if (payload === "setup") {
        this.settingsPage = "collection";
        void this.navigate(
          this.statusError ||
            this.status?.running ||
            this.status?.service_installed
            ? "settings"
            : "onboarding",
        );
      } else if (
        ["dashboard", "settings", "connection", "workspace"].includes(payload)
      )
        void this.navigate(payload as View);
    });
    const keys = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === ",") {
        e.preventDefault();
        void this.navigate("settings");
      }
    };
    window.addEventListener("keydown", keys);
    const timers = [
      setInterval(() => {
        if (!this.collectionAction) void this.refreshStatus();
      }, 5000),
      setInterval(() => void this.refreshUpdates(), 2000),
    ];
    void this.start();
    return () => {
      this.disposed = true;
      timers.forEach(clearInterval);
      cleanups.forEach((off) => off());
      window.removeEventListener("keydown", keys);
    };
  }
}
