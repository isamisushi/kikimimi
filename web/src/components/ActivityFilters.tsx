import { NativeSelect } from "@kikimimi/ui";
import { t } from "../i18n";
import { request } from "../api/client";
import { useAsync } from "../hooks/useAsync";
import { useSession } from "../hooks/useSession";
import { setActivityFilter, useActivityFilter } from "../activityFilter";

type Options = { this_host_id: string; devices: string[]; repositories: string[] };
export function ActivityFilters() {
  const filter = useActivityFilter();
  const { session } = useSession();
  const options = useAsync(() => request<Options>("/web/q/filter-options"), []);
  const data = options.status === "ok" ? options.data : null;
  const devices = data?.devices.filter((id) => id !== data.this_host_id) ?? [];
  const repos = data?.repositories ?? [];
  return (
    <section className="activity-filters" aria-label={t("Activity filters")}>
      <label>
        <span>{t("Device")}</span>
        <NativeSelect aria-label={t("Device")} value={filter.device}
          onChange={(event) => setActivityFilter({ ...filter, device: event.target.value })}>
          <option value="all">{t(session?.data_source === "s3" ? "All devices in this source" : "All my devices")}</option>
          <option value="this-mac">{t("This Mac only")}</option>
          {devices.map((id) => <option key={id} value={id}>{id}</option>)}
          {filter.device !== "all" && filter.device !== "this-mac" && !devices.includes(filter.device) &&
            <option value={filter.device}>{filter.device} ({t("Not in current data")})</option>}
        </NativeSelect>
      </label>
      <label>
        <span>{t("Repository")}</span>
        <NativeSelect aria-label={t("Repository")} value={filter.repository}
          onChange={(event) => setActivityFilter({ ...filter, repository: event.target.value })}>
          <option value="all">{t("All repositories")}</option>
          <option value="none">{t("No repository")}</option>
          {repos.map((repo) => <option key={repo} value={`repo:${repo}`}>{repo}</option>)}
          {filter.repository.startsWith("repo:") && !repos.includes(filter.repository.slice(5)) &&
            <option value={filter.repository}>{filter.repository.slice(5)} ({t("Not in current data")})</option>}
        </NativeSelect>
      </label>
      {(filter.device !== "all" || filter.repository !== "all") &&
        <button className="btn btn--ghost" onClick={() => setActivityFilter({ device: "all", repository: "all" })}>{t("Clear filters")}</button>}
      {options.status === "error" && <p role="alert">{t("Could not load filter options.")} <button className="btn btn--ghost" onClick={options.reload}>{t("Retry")}</button></p>}
    </section>
  );
}
