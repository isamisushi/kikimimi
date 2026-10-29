import type { View, Connection, CollectionChange } from "./types.ts";
// Three independent application boundaries. Browsing has no write capabilities
// for connection configuration or collection destinations.
export class Browsing {
  ports: {
    history: (reader: string, page?: string | null) => Promise<void>;
    cloud: (page?: string | null) => Promise<void>;
    hide: () => Promise<void>;
  };
  destination: View;
  reader: string;
  revision: number;
  constructor(
    ports: Browsing["ports"],
    saved: { destination?: View; reader?: string } = {},
  ) {
    this.ports = ports;
    this.destination = saved.destination || "dashboard";
    this.reader = saved.reader === "s3" ? "s3" : "local";
    this.revision = 0;
  }
  select(destination: View, reader = this.reader) {
    this.destination = destination;
    this.reader = reader;
    return ++this.revision;
  }
  current(revision: number) {
    return revision === this.revision;
  }
  snapshot() {
    return { destination: this.destination, reader: this.reader };
  }
  async present(revision: number, page?: string | null) {
    if (!this.current(revision)) return;
    if (this.destination === "cloud") return this.ports.cloud(page);
    if (this.destination === "dashboard")
      return this.ports.history(this.reader, page);
    return this.ports.hide();
  }
}

export class Connections {
  ports: {
    s3: (selection: Connection) => Promise<void>;
    cloud: (id: string) => Promise<void>;
  };
  saved: Record<string, Connection>;
  constructor(
    ports: Connections["ports"],
    saved: Record<string, Connection> = {},
  ) {
    this.ports = ports;
    this.saved = structuredClone(saved);
  }
  async configure(id: string, selection: Connection) {
    if (selection.kind === "cloud") await this.ports.cloud(id);
    else if (selection.kind === "s3") await this.ports.s3(selection);
    else if (selection.kind !== "local") throw new Error("Unknown connection");
    // Commit only after the connection has been validated successfully.
    this.saved[id] = structuredClone(selection);
  }
}

export class CollectionDestination {
  port: (selection: CollectionChange) => Promise<void>;
  constructor(port: CollectionDestination["port"]) {
    this.port = port;
  }
  async change(selection: CollectionChange) {
    if (!selection.expected)
      throw new Error("Refresh the collection destination before changing it.");
    return this.port(structuredClone(selection));
  }
}
