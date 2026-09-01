import { useState } from "react";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import { FreshnessBadge } from "../components/FreshnessBadge";
import { fmtDateTime } from "../api/format";
import * as api from "../api/client";
import type { Device } from "../api/types";

export function Devices() {
  const devices = useAsync(() => api.getDevices(), []);
  const [revokingId, setRevokingId] = useState<string | null>(null);

  async function onRevoke(id: string) {
    setRevokingId(id);
    try {
      await api.revokeDevice(id);
      devices.reload();
    } finally {
      setRevokingId(null);
    }
  }

  const columns: ColumnDef<Device>[] = [
    {
      key: "hostname",
      label: "Device",
      sortValue: (d) => d.hostname ?? d.host_id,
      render: (d) => (
        <span>
          {d.hostname ?? <span className="mono">{d.host_id}</span>}
          {d.revoked && <span className="badge badge--warn">Revoked</span>}
        </span>
      ),
    },
    {
      key: "account_email",
      label: "Owner",
      sortValue: (d) => d.account_email,
      render: (d) => d.account_email,
    },
    {
      key: "org_slug",
      label: "Org",
      sortValue: (d) => d.org_slug,
      render: (d) => <span className="badge badge--neutral">{d.org_slug}</span>,
    },
    {
      key: "last_seen_at",
      label: "Last seen",
      sortValue: (d) => (d.last_seen_at ? new Date(d.last_seen_at).getTime() : null),
      render: (d) => <FreshnessBadge lastEventTs={d.last_seen_at} />,
    },
    {
      key: "created_at",
      label: "Registered",
      sortValue: (d) => new Date(d.created_at).getTime(),
      render: (d) => fmtDateTime(d.created_at),
    },
    {
      key: "revoke",
      label: "",
      render: (d) =>
        d.revoked ? null : (
          <button
            type="button"
            className="btn btn--ghost btn--small"
            disabled={revokingId === d.id}
            onClick={() => void onRevoke(d.id)}
          >
            {revokingId === d.id ? "Revoking…" : "Revoke"}
          </button>
        ),
    },
  ];

  return (
    <div className="page">
      <div className="page__header">
        <h1>Devices</h1>
        <p className="page__subtitle">
          Machines registered via <code className="mono">kikimimi login</code>. If you're an admin of the
          active org, every device in that org is shown; otherwise, only your own (across all your orgs).
        </p>
      </div>

      <section className="panel">
        <QueryBoundary
          state={devices}
          isEmpty={(d) => d.devices.length === 0}
          emptyLabel="No devices registered yet"
          onRetry={devices.reload}
        >
          {(data) => (
            <SortableTable
              columns={columns}
              rows={data.devices}
              rowKey={(d) => d.id}
              defaultSortKey="last_seen_at"
              rowClassName={(d) => (d.revoked ? "row-warn" : undefined)}
              caption="Registered devices: owner, org, last seen, registration date, and a revoke action."
            />
          )}
        </QueryBoundary>
      </section>
    </div>
  );
}
