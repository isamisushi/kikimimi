import { configureTransport } from "../../web/src/api/client";
import {
  Browsing,
  Connections,
  CollectionDestination,
} from "./workspace-domain";
import type {
  View,
  Source,
  Connection,
  Org,
  Status,
  Updates,
  RouterStatus,
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
  if (!target) return "Unknown destination";
  return (
    [
      target.cloud &&
        (target.cloud.org_kind === "personal"
          ? "Personal (Cloud)"
          : `${target.cloud.org_slug} (Cloud)`),
      target.s3 && `S3 · ${target.s3.url}`,
    ]
      .filter(Boolean)
      .join(" + ") || "This Mac"
  );
}
export function activityText(timestamp?: number) {
  if (!timestamp) return "Waiting for agent activity";
  const minutes = Math.max(0, Math.floor((Date.now() - timestamp) / 60000));
  return minutes < 1
    ? "Last activity just now"
    : minutes < 60
      ? `Last activity ${minutes} min ago`
      : `Last activity ${new Date(timestamp).toLocaleString()}`;
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
  lastDashboard: "dashboard" | "cloud" = "dashboard";
  preferences: Record<string, Connection> = {};
  orgs: Org[] = [];
  signingIn = false;
  browseOnly = false;
  message: { text: string; error: boolean } | null = null;
  sourceBusy = false;
  collectionAction: string | null = null;
  collectionSwitch = false;
  updateAction: string | null = null;
  settingsPage: "collection" | "codex" | "general" = "collection";
  storage: Storage | null = null;
  storageError = "";
  storageBusy = false;
  router: RouterStatus | null = null;
  routerBusy = false;
  routerError = "";
  consent = false;
  disconnectOpen = false;
  collectionReview: CollectionChange | null = null;
  collectionConsent = false;
  collectionRepos = "";
  collectionAll = false;
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
  cloudRequest: Promise<{ orgs: Org[]; active_org: string } | null> | null =
    null;
  historySource: "local" | "s3" = "local";
  historyReady = false;
  historyKey = 0;
  started = false;
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
      preferences?: Record<string, Connection>;
    }>("workspaces-v1", {});
    this.workspace = legacy.active || "personal";
    this.preferences = legacy.preferences || {};
    if (!this.preferences.personal)
      this.preferences.personal = {
        kind: (localStorage.getItem("data-source") || "local") as Source,
      };
    const saved = parse<{
      destination?: View;
      reader?: Source;
      workspace?: string;
    }>("browsing-v1", {});
    this.reader =
      saved.destination === "cloud"
        ? "cloud"
        : saved.reader || this.preference().kind;
    this.lastDashboard = this.reader === "cloud" ? "cloud" : "dashboard";
    this.workspace =
      this.reader === "local" ? "personal" : saved.workspace || this.workspace;
    this.browsing = new Browsing(
      {
        history: async (source) => {
          await invoke("hide_dashboard");
          if (this.historySource !== source) this.historyKey++;
          this.historySource = source as "local" | "s3";
          this.historyReady = true;
          this.emit();
        },
        cloud: (page) => invoke("show_cloud", page ? { page } : undefined),
        hide: () => invoke("hide_dashboard"),
      },
      { destination: this.lastDashboard, reader: this.reader },
    );
    this.connections = new Connections(
      {
        s3: (selection) => invoke("set_source", { selection }),
        cloud: async (id) => {
          const session = await this.refreshCloud();
          const org = this.orgs.find((o) =>
            id === "personal"
              ? o.kind === "personal"
              : `cloud:${o.slug}` === id,
          );
          if (org && session?.active_org !== org.slug)
            await invoke("select_cloud_workspace", { slug: org.slug });
          else if (!org && (session || id.startsWith("cloud:")))
            throw Error(
              "Your Cloud workspace is unavailable. Refresh your team list.",
            );
          this.signingIn = !session;
        },
      },
      this.preferences,
    );
    this.preferences = this.connections.saved;
    this.collectionDestination = new CollectionDestination((selection) =>
      invoke("switch_collection", { selection }),
    );
  }
  preference(id = this.workspace): Connection {
    return id.startsWith("cloud:")
      ? { kind: "cloud" }
      : id === "s3-team"
        ? { ...this.preferences[id], kind: "s3" }
        : this.preferences[id] || { kind: "local" };
  }
  workspaceName(id = this.workspace) {
    return id === "personal"
      ? "Personal"
      : id === "s3-team"
        ? "Team · S3"
        : this.orgs.find((o) => `cloud:${o.slug}` === id)?.name || "Cloud team";
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
      ? "Unable to check collection"
      : this.collectionSwitch
        ? "Changing collection…"
        : this.collectionAction === "enable"
          ? "Setting up collection…"
          : this.collectionAction === "resume"
            ? "Resuming collection…"
            : this.collectionAction === "disconnect"
              ? "Disconnecting…"
              : this.pending
                ? "Collection destination pending"
                : !s
                  ? "Checking collection…"
                  : `${s.running ? "Collecting" : !s.service_installed && !s.has_history ? "Not set up" : "Collection stopped"} · ${targetName(this.target)}`;
  }
  get statusAction() {
    return this.statusError
      ? "Retry"
      : this.status?.running
        ? ""
        : this.status?.service_installed
          ? "Resume"
          : this.view === "onboarding"
            ? ""
            : "Set up collection";
  }
  notice(text: string | null, error = false) {
    this.message = text ? { text, error } : null;
    this.emit();
  }
  persistBrowsing() {
    localStorage.setItem(
      "browsing-v1",
      JSON.stringify({
        destination: this.lastDashboard,
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
    if (this.reader === "cloud")
      localStorage.setItem("cloud-workspace", this.workspace);
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
  async refreshCloud(adopt = false) {
    if (this.cloudRequest) return this.cloudRequest;
    this.cloudRequest = (async () => {
      const session = await this.invoke<{
        orgs: Org[];
        active_org: string;
      } | null>("cloud_workspaces");
      this.orgs = session?.orgs || [];
      if (adopt && this.view === "cloud" && session) {
        const active = this.orgs.find((o) => o.slug === session.active_org);
        if (active) {
          this.workspace =
            active.kind === "personal" ? "personal" : `cloud:${active.slug}`;
          this.signingIn = false;
          localStorage.setItem("cloud-workspace", this.workspace);
        }
      }
      this.emit();
      return session;
    })();
    try {
      return await this.cloudRequest;
    } finally {
      this.cloudRequest = null;
    }
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
  navigate(next: View, page?: string) {
    if (this.sourceBusy || this.collectionSwitch) return this.navigation;
    if (next === "dashboard" && this.reader === "s3" && !this.preference().url)
      next = "connection";
    if (next === "cloud") this.browseOnly = true;
    if (next === "dashboard" || next === "cloud") this.lastDashboard = next;
    if (next === "dashboard" && !this.knownSetup) next = "onboarding";
    if (next === "onboarding") this.consent = false;
    this.view = next;
    this.viewerError = false;
    this.viewerTitle =
      next === "cloud" ? "Opening Cloud…" : "Opening your dashboard…";
    this.viewerDetail =
      next === "cloud"
        ? "Connecting to your Cloud workspace."
        : "Loading your saved history.";
    const revision = this.browsing.select(
      next,
      this.reader === "s3" ? "s3" : this.browsing.reader,
    );
    this.persistBrowsing();
    const hidden = this.invoke("hide_dashboard").then(
      () => null,
      (e) => e,
    );
    this.emit();
    this.navigation = this.navigation
      .catch(() => {})
      .then(async () => {
        if (!this.browsing.current(revision)) return;
        try {
          const error = await hidden;
          if (error) throw error;
          if (!this.browsing.current(revision)) return;
          if (next === "dashboard" || next === "cloud")
            await this.browsing.present(revision, page);
          else if (next === "connection") await this.refreshSource();
          else if (next === "settings") {
            void this.refreshRouter();
            await this.refreshStorage();
          } else if (next === "workspace") {
            void this.refreshCloud().catch(() => {});
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
  async openCloud() {
    if (
      this.sourceBusy ||
      this.collectionSwitch ||
      (this.view === "cloud" && !this.viewerError && !this.message?.error)
    )
      return;
    this.reader = "cloud";
    this.message = null;
    this.workspace = localStorage.getItem("cloud-workspace") || "personal";
    await this.navigate("cloud", "overview");
    void this.refreshCloud().catch(() => {});
  }
  async connectAndView(id: string, selection: Connection) {
    await this.connections.configure(id, selection);
    this.workspace = id;
    this.reader = selection.kind;
    this.signingIn = selection.kind === "cloud" && this.signingIn;
    this.lastDashboard = selection.kind === "cloud" ? "cloud" : "dashboard";
    this.view = this.lastDashboard;
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
    await this.browsing.present(
      revision,
      selection.kind === "cloud" ? "overview" : null,
    );
  }
  async changeWorkspace(id: string) {
    if (this.sourceBusy || this.collectionSwitch) return;
    if (id === "signin") {
      await this.navigate("connection");
      this.sourceBusy = true;
      this.emit();
      try {
        await this.refreshCloud();
        if (!this.orgs.length) {
          this.signingIn = true;
          this.view = "cloud";
          await this.browsing.present(this.browsing.select("cloud"));
          this.notice("Sign in to choose a Cloud workspace.");
        } else this.notice("Team list refreshed. Choose a workspace above.");
      } catch {
        this.notice(
          "Could not open Cloud. Check your connection and retry.",
          true,
        );
      } finally {
        this.sourceBusy = false;
        this.emit();
      }
      return;
    }
    const scope = ["workspace", "settings", "connection"].includes(this.view)
      ? this.view
      : null;
    this.sourceBusy = true;
    this.emit();
    try {
      await this.navigation;
      const selection = this.preference(id);
      if (selection.kind === "s3" && !selection.url) {
        await this.invoke("hide_dashboard");
        this.workspace = id;
        this.reader = "s3";
        this.lastDashboard = "dashboard";
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
      await this.invoke("hide_dashboard").catch(() => {});
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
      await this.invoke("hide_dashboard").catch(() => {});
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
    try {
      await this.invoke(command);
      await this.refreshStatus();
      if (command === "enable") {
        this.consent = false;
        this.notice(
          "Collection is on. Restart existing agent sessions to load the new settings.",
        );
        await this.navigate(this.lastDashboard);
      } else
        this.notice(
          command === "disconnect"
            ? "Disconnected. Your saved history is still available."
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
  choices(): Choice[] {
    const choices: Choice[] = [
      { id: "local", label: "This Mac only", kind: "local" },
    ];
    for (const org of this.orgs)
      choices.push({
        id: `cloud:${org.slug}`,
        label: `${org.kind === "personal" ? "Personal" : org.name} · Cloud`,
        kind: "cloud",
        slug: org.slug,
        team: org.kind === "team",
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
    this.collectionReview = {
      ...choice,
      expected: structuredClone(this.status.collection_target),
    };
    this.collectionConsent = false;
    this.collectionRepos = "";
    this.collectionAll = false;
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
    const repos = review.team
      ? this.collectionAll
        ? ["*"]
        : this.collectionRepos
            .split(/\n/)
            .map((s) => s.trim())
            .filter(Boolean)
      : [];
    if (review.team && !repos.length) {
      this.collectionError =
        "Choose the repositories to share, or explicitly select all repositories.";
      this.emit();
      return;
    }
    this.collectionSwitch = true;
    this.emit();
    try {
      await this.collectionDestination.change({
        ...review,
        repo_patterns: repos,
      });
      this.collectionReview = null;
      await this.refreshStatus();
      await this.refreshStorage();
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
          ? "Codex now uses your team’s router. Restart Codex to apply."
          : "Codex is back on its previous provider. Restart Codex to apply.",
      );
    this.emit();
  }
  async manageTeam() {
    this.sourceBusy = true;
    this.emit();
    try {
      await this.connections.configure(this.workspace, { kind: "cloud" });
      await this.browsing.present(this.browsing.select("cloud"), "team");
      this.view = "cloud";
    } catch {
      this.notice(
        "Could not open team settings. Check your connection and try again.",
        true,
      );
    } finally {
      this.sourceBusy = false;
      this.emit();
    }
  }
  async statusActionClick() {
    if (this.statusError) {
      await this.refreshStatus();
      if (this.view === "loading" && !this.statusError)
        await this.navigate(this.knownSetup ? "dashboard" : "onboarding");
    } else if (this.status?.service_installed) await this.collection("resume");
    else await this.navigate("onboarding");
  }
  async resumeEmail() {
    if (this.started && (await this.invoke<boolean>("take_email_return"))) {
      this.signingIn = true;
      await this.navigate("cloud");
    }
  }
  async start() {
    this.disposed = false;
    await this.refreshStatus();
    if (this.disposed) return;
    await this.refreshSource();
    if (this.reader === "cloud") {
      const session = await this.refreshCloud().catch(() => null);
      if (!session) {
        this.reader = "local";
        this.workspace = "personal";
        this.lastDashboard = "dashboard";
        this.browsing.reader = "local";
      }
    } else void this.refreshCloud().catch(() => {});
    if (this.disposed) return;
    if (this.reader === "s3" && !this.preference().url)
      await this.navigate("connection");
    else if (!this.statusError && this.view === "loading")
      await this.navigate(
        this.lastDashboard === "cloud"
          ? "cloud"
          : this.knownSetup
            ? "dashboard"
            : "onboarding",
      );
    void this.refreshUpdates();
    this.started = true;
    await this.resumeEmail();
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
    listen("email-return", () => void this.resumeEmail());
    for (const dest of ["cloud", "dashboard"] as const)
      listen(`${dest}-load-error`, () => {
        if (this.view !== dest) return;
        this.viewerError = true;
        this.viewerTitle =
          dest === "cloud"
            ? "Couldn’t load Cloud"
            : "Couldn’t load your history";
        this.viewerDetail = "The page did not finish loading. Try again.";
        this.emit();
      });
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
        ["dashboard", "cloud", "settings", "connection", "workspace"].includes(
          payload,
        )
      )
        void this.navigate(
          payload === "dashboard" ? this.lastDashboard : (payload as View),
        );
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
      setInterval(() => {
        if (!this.sourceBusy && this.view === "cloud")
          void this.refreshCloud(this.signingIn).catch(() => {});
      }, 10000),
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
