export type View =
  | "loading"
  | "onboarding"
  | "dashboard"
  | "cloud"
  | "settings"
  | "connection"
  | "workspace";
export type Source = "local" | "s3" | "cloud";
export type Connection = {
  kind: Source;
  url?: string;
  profile?: string | null;
  endpoint_url?: string | null;
};
export type Org = { slug: string; name: string; kind: string; role?: string };
export type Destination = {
  cloud: {
    org_slug: string;
    org_kind: string;
    hosted?: boolean;
    repo_patterns?: string[];
  } | null;
  s3: Omit<Connection, "kind"> | null;
};
export type Status = {
  running: boolean;
  service_installed: boolean;
  has_history: boolean;
  has_s3_reader?: boolean;
  last_event_ts?: number;
  collection_target?: Destination;
  applied_collection_target?: Destination;
};
export type Updates = {
  configured: boolean;
  phase: string;
  current_version?: string;
  available_version?: string | null;
  auto_update?: boolean;
  error?: string | null;
  error_kind?: string;
};
export type RouterStatus = {
  enabled: boolean;
  applied: boolean;
  logged_in: boolean;
  org_kind?: string;
  org_slug?: string;
  codex_found: boolean;
};
export type Storage = {
  local_path: string;
  cloud: Destination["cloud"];
  s3: Destination["s3"];
};
export type Choice = {
  id: string;
  label: string;
  kind: Source;
  slug?: string;
  team?: boolean;
  s3?: Destination["s3"];
};
export type CollectionChange = Choice & {
  expected: Destination;
  repo_patterns?: string[];
};
export type NativeCommand =
  | "status"
  | "update_status"
  | "cloud_workspaces"
  | "read_storage"
  | "read_source"
  | "codex_router_status"
  | "history_request"
  | "show_cloud"
  | "hide_dashboard"
  | "set_source"
  | "select_cloud_workspace"
  | "switch_collection"
  | "enable"
  | "resume"
  | "disconnect"
  | "check_updates"
  | "install_update"
  | "set_auto_update"
  | "set_codex_router"
  | "take_email_return";
export type Invoke = <T = void>(
  command: NativeCommand,
  args?: Record<string, unknown>,
) => Promise<T>;
declare global {
  interface Window {
    __TAURI__: {
      core: { invoke: Invoke };
      event?: {
        listen: (
          name: string,
          fn: (event: { payload: string }) => void,
        ) => Promise<() => void> | void;
      };
    };
  }
}
