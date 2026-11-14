//! The local durable event ledger and transactional cloud outbox.
//! One collector process owns the connection. Consumers query resolved_events;
//! transports acknowledge deliveries, never mutate the source observations.
use anyhow::{ensure, Context, Result};
use duckdb::{params, Connection};
use kikimimi_schema::Event;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub type SharedStore = std::sync::Arc<std::sync::Mutex<Store>>;
static STORES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, SharedStore>>,
> = std::sync::OnceLock::new();
pub fn registered(directory: &Path) -> Option<SharedStore> {
    STORES.get()?.lock().ok()?.get(directory).cloned()
}
pub fn shared(directory: &Path) -> Result<SharedStore> {
    let mut stores = STORES
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| anyhow::anyhow!("store registry poisoned"))?;
    if let Some(store) = stores.get(directory) {
        return Ok(store.clone());
    }
    let mut database = Store::open(&directory.join("kikimimi.duckdb"))?;
    database.import_history(directory)?;
    let store = std::sync::Arc::new(std::sync::Mutex::new(database));
    stores.insert(directory.to_owned(), store.clone());
    Ok(store)
}
pub fn registered_query(
    sql: &str,
) -> Option<Result<Vec<serde_json::Map<String, serde_json::Value>>>> {
    let (store, sql) = {
        let stores = STORES.get()?.lock().ok()?;
        stores.iter().find_map(|(directory, store)| {
            let glob = kikimimi_schema::paths::events_glob_sql_in(directory);
            let source =
                format!("read_parquet('{glob}', union_by_name=true, hive_partitioning=false)");
            sql.contains(&source)
                .then(|| (store.clone(), sql.replace(&source, "raw_events")))
        })?
    };
    // Never hold the registry mutex while waiting for the writer or executing SQL.
    // HTTP handlers also consult the registry on the async runtime's threads.
    Some(query_shared(&store, &sql))
}

pub fn query_shared(
    store: &SharedStore,
    sql: &str,
) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
    let reader = query_reader(store)?;
    // DuckDB connections share the same live database, with statement snapshots.
    // Collection and other dashboard queries must not wait for this entire scan.
    query_connection(&reader, sql)
}

fn query_reader(store: &SharedStore) -> Result<Connection> {
    let writer = store
        .lock()
        .map_err(|_| anyhow::anyhow!("store poisoned"))?;
    Ok(writer.connection.try_clone()?)
}

fn query_connection(
    connection: &Connection,
    sql: &str,
) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
    let sql = sql.trim().trim_end_matches(';');
    let mut statement = connection.prepare(&format!(
        "SELECT CAST(to_json(q) AS VARCHAR) FROM ({sql}) q"
    ))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
}

pub struct Store {
    connection: Connection,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delivery {
    pub id: String,
    pub payload: Event,
    pub share_with_team: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub sequence: i64,
    pub event: Event,
    pub deleted: bool,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Some(parent) = path.parent() {
            ensure!(
                !std::fs::symlink_metadata(parent)?.file_type().is_symlink(),
                "database directory must not be a symlink"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        if path.exists() {
            ensure!(
                std::fs::symlink_metadata(path)?.file_type().is_file(),
                "database must be a regular file"
            );
        }
        let connection = Connection::open(path).context("opening local event database")?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS observations (
            scope VARCHAR NOT NULL, event_id VARCHAR NOT NULL, payload JSON NOT NULL,
            PRIMARY KEY(scope,event_id));
          CREATE TABLE IF NOT EXISTS sharing_decisions (event_id VARCHAR PRIMARY KEY, destination VARCHAR);
          CREATE TABLE IF NOT EXISTS outbox (
            destination VARCHAR NOT NULL, event_id VARCHAR NOT NULL, payload JSON NOT NULL, share_with_team BOOLEAN NOT NULL DEFAULT false,
            PRIMARY KEY(destination,event_id));
          CREATE TABLE IF NOT EXISTS receipts (
            destination VARCHAR NOT NULL, event_id VARCHAR NOT NULL,
            PRIMARY KEY(destination,event_id));
          CREATE TABLE IF NOT EXISTS device_event_aliases (
            scope VARCHAR NOT NULL, destination VARCHAR NOT NULL, local_id VARCHAR NOT NULL,
            remote_id VARCHAR NOT NULL, PRIMARY KEY(scope,destination,local_id),
            UNIQUE(scope,destination,remote_id));
          CREATE TABLE IF NOT EXISTS sync_cursors (scope VARCHAR PRIMARY KEY, position BIGINT NOT NULL);
          CREATE TABLE IF NOT EXISTS tombstones (scope VARCHAR NOT NULL, event_id VARCHAR NOT NULL,
            PRIMARY KEY(scope,event_id));")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        let mut store = Self { connection };
        store.activate(None)?;
        Ok(store)
    }
    pub fn activate(&mut self, scope: Option<&str>) -> Result<()> {
        let scope = scope.unwrap_or("").replace('\'', "''");
        let fields = kikimimi_schema::COLUMNS
            .iter()
            .map(|name| {
                let ty = match *name {
                    "ts"
                    | "duration_ms"
                    | "input_tokens"
                    | "output_tokens"
                    | "cache_read_tokens"
                    | "cache_write_tokens"
                    | "reasoning_tokens"
                    | "estimated_input_tokens"
                    | "estimated_output_tokens" => "BIGINT",
                    "cost_usd" => "DOUBLE",
                    "success" | "thinking" | "redaction_applied" => "BOOLEAN",
                    _ => "VARCHAR",
                };
                format!("CAST(json_extract_string(payload,'$.{name}') AS {ty}) AS {name}")
            })
            .collect::<Vec<_>>()
            .join(",");
        self.connection.execute_batch(&format!("CREATE OR REPLACE VIEW raw_events AS SELECT {fields} FROM (SELECT payload FROM observations o WHERE (scope='local' OR scope='{scope}') AND NOT EXISTS (SELECT 1 FROM tombstones t WHERE t.scope='{scope}' AND t.event_id=o.event_id) \
            AND NOT (o.scope='local' AND EXISTS (SELECT 1 FROM device_event_aliases a WHERE a.scope='{scope}' AND a.local_id=o.event_id \
              AND (EXISTS (SELECT 1 FROM observations r WHERE r.scope=a.scope AND r.event_id=a.remote_id) \
                OR EXISTS (SELECT 1 FROM tombstones t WHERE t.scope=a.scope AND t.event_id=a.remote_id)))) \
            QUALIFY row_number() OVER (PARTITION BY event_id ORDER BY (scope='local') DESC)=1)"))?;
        self.connection.execute_batch(&format!(
            "CREATE OR REPLACE VIEW resolved_events AS SELECT * FROM {}",
            resolved_source("raw_events")
        ))?;
        Ok(())
    }

    /// The same resolved relation is consumed by the UI and CLI. Statements are
    /// wrapped as a subquery, so callers cannot submit mutations or multi-statements.
    pub fn query(&self, sql: &str) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        query_connection(&self.connection, sql)
    }

    pub fn contains_local(&self, id: &str) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT count(*) FROM observations WHERE scope='local' AND event_id=?",
            [id],
            |r| r.get::<_, i64>(0),
        )? > 0)
    }

    pub fn client_ids(&self, client: &str) -> Result<std::collections::HashSet<String>> {
        let mut statement=self.connection.prepare("SELECT event_id FROM observations WHERE scope='local' AND json_extract_string(payload,'$.client')=?")?;
        Ok(statement
            .query_map([client], |r| r.get::<_, String>(0))?
            .collect::<duckdb::Result<_>>()?)
    }
    pub fn checkpoint(&self) -> Result<()> {
        self.connection.execute_batch("CHECKPOINT")?;
        Ok(())
    }

    /// One-time cutover; archived files remain untouched. No historical data is
    /// shared implicitly. The marker and imported observations commit together.
    pub fn import_history(&mut self, directory: &Path) -> Result<()> {
        self.connection
            .execute_batch("CREATE TABLE IF NOT EXISTS imports (name VARCHAR PRIMARY KEY)")?;
        let done: i64 = self.connection.query_row(
            "SELECT count(*) FROM imports WHERE name='parquet'",
            [],
            |r| r.get(0),
        )?;
        if done > 0 {
            return Ok(());
        }
        let tx = self.connection.transaction()?;
        tx.execute_batch("CREATE TEMP TABLE history_stage(event_id VARCHAR,payload VARCHAR)")?;
        {
            let mut appender = tx.appender("history_stage")?;
            crate::repair::visit_history(directory, |event| {
                ensure!(!event.event_id.is_empty(), "historical event has no ID");
                appender.append_row(params![event.event_id, serde_json::to_string(&event)?])?;
                Ok(())
            })?;
            appender.flush()?;
        }
        tx.execute_batch("INSERT INTO observations SELECT 'local',event_id,CAST(payload AS JSON) FROM history_stage QUALIFY row_number() OVER (PARTITION BY event_id)=1 ON CONFLICT DO NOTHING;
            INSERT INTO sharing_decisions SELECT event_id,NULL FROM observations WHERE scope='local' ON CONFLICT DO NOTHING;
            DROP TABLE history_stage;")?;
        tx.execute("INSERT INTO imports VALUES ('parquet')", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Explicitly authorized historical upload; downloads never enter this queue.
    pub fn enqueue_history(
        &mut self,
        destination: &str,
        share: impl Fn(&Event) -> bool,
    ) -> Result<usize> {
        let events: Vec<Event> = {
            let mut statement = self
                .connection
                .prepare("SELECT CAST(o.payload AS VARCHAR) FROM observations o WHERE o.scope='local' AND NOT EXISTS (SELECT 1 FROM receipts r WHERE r.destination=? AND r.event_id=o.event_id) AND NOT EXISTS (SELECT 1 FROM outbox b WHERE b.destination=? AND b.event_id=o.event_id)")?;
            let rows = statement.query_map([destination, destination], |r| r.get::<_, String>(0))?;
            rows.map(|r| Ok(serde_json::from_str(&r?)?))
                .collect::<Result<_>>()?
        };
        let tx = self.connection.transaction()?;
        let mut count = 0;
        for mut event in events {
            let shared = share(&event);
            crate::cloud::mask_for_cloud(&mut event);
            count+=tx.execute("INSERT INTO outbox(destination,event_id,payload,share_with_team) SELECT ?,?,?,? WHERE NOT EXISTS (SELECT 1 FROM receipts WHERE destination=? AND event_id=?) ON CONFLICT DO NOTHING",params![destination,event.event_id,serde_json::to_string(&event)?,shared,destination,event.event_id])?;
        }
        tx.commit()?;
        Ok(count)
    }

    pub fn pending_count(&self, destination: &str) -> Result<usize> {
        Ok(self.connection.query_row(
            "SELECT count(*) FROM outbox WHERE destination=?",
            [destination],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    /// Commit source data and its sharing decision atomically. A replay cannot
    /// re-queue an acknowledged event or move it to another workspace.
    pub fn ingest(&mut self, event: &Event, delivery: Option<(&str, &Event)>) -> Result<()> {
        self.ingest_with_sharing(event, delivery, false)
    }
    pub fn ingest_with_sharing(
        &mut self,
        event: &Event,
        delivery: Option<(&str, &Event)>,
        share_with_team: bool,
    ) -> Result<()> {
        ensure!(!event.event_id.is_empty(), "missing event identity");
        let tx = self.connection.transaction()?;
        let payload = serde_json::to_string(event)?;
        let previous:Option<String>=tx.query_row("SELECT CAST(payload AS VARCHAR) FROM observations WHERE scope='local' AND event_id=?",[&event.event_id],|r|r.get(0)).optional()?;
        if let Some(previous) = previous {
            let stored = serde_json::from_str::<Event>(&previous)?;
            if stored != *event {
                let stored = serde_json::to_value(stored)?;
                let incoming = serde_json::to_value(event)?;
                let fields: Vec<_> = incoming
                    .as_object()
                    .unwrap()
                    .iter()
                    .filter(|(key, value)| stored.get(key) != Some(*value))
                    .map(|(key, _)| key.as_str())
                    .collect();
                anyhow::bail!(
                    "conflicting event identity {} ({}/{}/{}): differing fields {}",
                    event.event_id,
                    event.agent,
                    event.source,
                    event.event_type,
                    fields.join(", ")
                );
            }
            // A replay keeps the original sharing decision, even after sign-out
            // or a workspace switch. It must neither requeue nor change owners.
            tx.commit()?;
            return Ok(());
        }
        tx.execute(
            "INSERT INTO observations VALUES ('local',?,?) ON CONFLICT DO NOTHING",
            params![event.event_id, payload],
        )?;
        tx.execute(
            "INSERT INTO sharing_decisions VALUES (?,?) ON CONFLICT DO NOTHING",
            params![event.event_id, delivery.map(|(scope, _)| scope)],
        )?;
        if let Some((destination, masked)) = delivery {
            let mut expected = event.clone();
            expected.prompt_text = None;
            expected.tool_input_json = None;
            expected.tool_output_excerpt = None;
            expected.redaction_applied = None;
            ensure!(
                *masked == expected,
                "delivery differs from redacted source observation"
            );
            ensure!(
                !destination.is_empty() && masked.event_id == event.event_id,
                "invalid delivery identity"
            );
            ensure!(
                masked.prompt_text.is_none()
                    && masked.tool_input_json.is_none()
                    && masked.tool_output_excerpt.is_none(),
                "cloud delivery contains content"
            );
            tx.execute("INSERT INTO outbox(destination,event_id,payload,share_with_team) SELECT ?,?,?,? WHERE NOT EXISTS (SELECT 1 FROM receipts WHERE destination=? AND event_id=?) ON CONFLICT DO NOTHING",params![destination,event.event_id,serde_json::to_string(masked)?,share_with_team,destination,event.event_id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// A verified predecessor observation already represents this delivery. Record
    /// coverage only after downloading its real ID for the authenticated account.
    pub fn acknowledge_predecessors(&mut self, scope: &str, destination: &str) -> Result<()> {
        let tx = self.connection.transaction()?;
        tx.execute(
            "INSERT INTO receipts
             SELECT b.destination,b.event_id FROM outbox b JOIN device_event_aliases a
             ON b.destination=a.destination AND b.event_id=a.local_id
             WHERE a.scope=? AND b.destination=? AND (
               EXISTS (SELECT 1 FROM observations r WHERE r.scope=a.scope AND r.event_id=a.remote_id)
               OR EXISTS (SELECT 1 FROM tombstones t WHERE t.scope=a.scope AND t.event_id=a.remote_id))
             ON CONFLICT DO NOTHING",
            params![scope, destination],
        )?;
        tx.execute(
            "DELETE FROM outbox b WHERE destination=? AND EXISTS
             (SELECT 1 FROM receipts r WHERE r.destination=b.destination AND r.event_id=b.event_id)",
            [destination],
        )?;
        let unresolved: i64 = tx.query_row(
            "SELECT count(*) FROM outbox b JOIN device_event_aliases a
             ON b.destination=a.destination AND b.event_id=a.local_id
             WHERE a.scope=? AND b.destination=?",
            params![scope, destination],
            |row| row.get(0),
        )?;
        ensure!(unresolved == 0, "predecessor observations missing from Cloud; upload deferred");
        tx.commit()?;
        Ok(())
    }
    pub fn pending(&self, destination: &str, limit: usize) -> Result<Vec<Delivery>> {
        let mut statement=self.connection.prepare("SELECT event_id,CAST(payload AS VARCHAR),share_with_team FROM outbox WHERE destination=? ORDER BY event_id LIMIT ?")?;
        let rows = statement.query_map(params![destination, limit.min(500) as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?;
        rows.map(|r| {
            let (id, payload, share_with_team) = r?;
            Ok(Delivery {
                id,
                payload: serde_json::from_str(&payload)?,
                share_with_team,
            })
        })
        .collect()
    }

    /// Only IDs explicitly acknowledged by the receiving service are retired.
    pub fn acknowledge(&mut self, destination: &str, ids: &[String]) -> Result<()> {
        let tx = self.connection.transaction()?;
        for id in ids {
            tx.execute("INSERT INTO receipts SELECT destination,event_id FROM outbox WHERE destination=? AND event_id=? ON CONFLICT DO NOTHING",params![destination,id])?;
            tx.execute(
                "DELETE FROM outbox WHERE destination=? AND event_id=?",
                params![destination, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn cursor(&self, scope: &str) -> Result<i64> {
        Ok(self
            .connection
            .query_row(
                "SELECT position FROM sync_cursors WHERE scope=?",
                [scope],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    /// Apply a complete server page and its cursor in one commit. Downloaded
    /// records never enter the upload queue, preventing sync loops.
    pub fn apply_page(
        &mut self,
        scope: &str,
        from: i64,
        through: i64,
        changes: &[Change],
    ) -> Result<()> {
        ensure!(
            scope != "local" && !scope.is_empty(),
            "invalid remote scope"
        );
        ensure!(
            self.cursor(scope)? == from && through >= from,
            "stale sync cursor"
        );
        let mut last = from;
        for change in changes {
            ensure!(
                change.sequence > last && change.sequence <= through,
                "unordered sync page"
            );
            last = change.sequence;
        }
        let tx = self.connection.transaction()?;
        for change in changes {
            let id = &change.event.event_id;
            if change.deleted {
                tx.execute(
                    "DELETE FROM observations WHERE scope=? AND event_id=?",
                    params![scope, id],
                )?;
                tx.execute(
                    "INSERT INTO tombstones VALUES (?,?) ON CONFLICT DO NOTHING",
                    params![scope, id],
                )?;
            } else {
                tx.execute("INSERT INTO observations SELECT ?,?,? WHERE NOT EXISTS (SELECT 1 FROM tombstones WHERE scope=? AND event_id=?) ON CONFLICT(scope,event_id) DO UPDATE SET payload=excluded.payload",params![scope,id,serde_json::to_string(&change.event)?,scope,id])?;
            }
        }
        tx.execute("INSERT INTO sync_cursors VALUES (?,?) ON CONFLICT(scope) DO UPDATE SET position=excluded.position",params![scope,through])?;
        tx.commit()?;
        Ok(())
    }
}
pub fn resolved_source(relation: &str) -> String {
    format!("(WITH raw AS MATERIALIZED (SELECT * FROM {relation}), \
      clients AS (SELECT host_id, session_id, min(client) AS client FROM raw \
        WHERE event_type='session.client' AND agent='codex' AND source='log' \
        AND correlation_confidence='observed_originator' AND client='chatgpt_desktop_cowork' \
        GROUP BY host_id,session_id), \
      links AS (SELECT host_id, parent_session_id, correlation_key, min(session_id) AS chat, min(turn_id) AS message \
        FROM raw link WHERE event_type='message.usage.link' AND correlation_confidence='content_match' \
        AND NOT EXISTS (SELECT 1 FROM raw rejected WHERE rejected.event_type='message.usage.link.rejected' \
          AND rejected.host_id=link.host_id AND rejected.session_id=link.parent_session_id AND rejected.correlation_key=link.correlation_key) \
        AND session_id IS NOT NULL AND turn_id IS NOT NULL \
        GROUP BY host_id,parent_session_id,correlation_key \
        HAVING count(DISTINCT session_id || ':' || turn_id)=1) \
      SELECT r.* REPLACE (coalesce(l.chat,r.session_id) AS session_id, \
        CASE WHEN r.agent='codex' AND (r.client IS NULL OR r.client='codex_desktop') THEN coalesce(c.client,r.client) ELSE r.client END AS client, \
        coalesce(l.message,r.turn_id) AS turn_id, \
        CASE WHEN l.chat IS NOT NULL THEN 'content_match' ELSE r.correlation_confidence END AS correlation_confidence) \
      FROM raw r LEFT JOIN links l ON r.event_type='api.request' AND r.client='claude_desktop' \
        AND r.host_id=l.host_id AND r.session_id=l.parent_session_id AND r.correlation_key=l.correlation_key \
      LEFT JOIN clients c ON r.host_id=c.host_id AND r.session_id=c.session_id \
      WHERE r.event_type NOT IN ('message.usage.link','message.usage.link.rejected','session.client'))")
}

use duckdb::OptionalExt;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waiting_query_does_not_block_registry_lookups() {
        let dir = tempfile::tempdir().unwrap();
        let store = shared(dir.path()).unwrap();
        let writer = store.lock().unwrap();
        let references = std::sync::Arc::strong_count(&store);
        let glob = kikimimi_schema::paths::events_glob_sql_in(dir.path());
        let query = std::thread::spawn(move || {
            registered_query(&format!(
                "SELECT count(*) AS n FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false)"
            )).unwrap().unwrap()
        });
        // Wait until the query has selected its store (or the old implementation
        // is holding the registry while blocked on our writer guard).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::sync::Arc::strong_count(&store) == references
            && STORES.get().unwrap().try_lock().is_ok()
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        let path = dir.path().to_owned();
        let (send, receive) = std::sync::mpsc::channel();
        let lookup = std::thread::spawn(move || {
            send.send(registered(&path).is_some()).unwrap();
        });
        let responsive = receive.recv_timeout(std::time::Duration::from_secs(1));
        drop(writer);
        query.join().unwrap();
        lookup.join().unwrap();
        assert_eq!(
            responsive.unwrap(),
            true,
            "query blocked an unrelated lookup"
        );
    }

    #[test]
    fn active_reader_allows_ingestion_and_other_queries_with_snapshot_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let store = shared(dir.path()).unwrap();
        store.lock().unwrap().ingest(&event(), None).unwrap();
        // Hold a real read transaction open while another thread commits data.
        // This uses the same reader acquisition as dashboard queries.
        let reader = query_reader(&store).unwrap();
        reader.execute_batch("BEGIN TRANSACTION").unwrap();
        let sql = "SELECT event_id FROM raw_events ORDER BY event_id";
        assert_eq!(query_connection(&reader, sql).unwrap().len(), 1);
        let other = store.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut next = event();
            next.event_id = "concurrent".into();
            other.lock().unwrap().ingest(&next, None).unwrap();
            send.send(query_shared(&other, sql).unwrap().len()).unwrap();
        });
        let result = receive.recv_timeout(std::time::Duration::from_secs(5));
        // The existing reader must retain its snapshot; a new query sees the commit.
        let snapshot = query_connection(&reader, sql).unwrap().len();
        reader.execute_batch("COMMIT").unwrap();
        worker.join().unwrap();
        assert_eq!(
            result.unwrap(),
            2,
            "writer or second query blocked by reader"
        );
        assert_eq!(snapshot, 1);
        assert_eq!(query_connection(&reader, sql).unwrap().len(), 2);
    }

    #[test]
    fn shared_queries_observe_committed_data_and_active_scope() {
        let dir = tempfile::tempdir().unwrap();
        let store = shared(dir.path()).unwrap();
        store.lock().unwrap().ingest(&event(), None).unwrap();
        let read =
            || query_shared(&store, "SELECT event_id FROM raw_events ORDER BY event_id").unwrap();
        assert_eq!(read().len(), 1);
        let mut remote = event();
        remote.event_id = "remote-event".into();
        {
            let mut writer = store.lock().unwrap();
            writer
                .apply_page(
                    "account",
                    0,
                    1,
                    &[Change {
                        sequence: 1,
                        event: remote,
                        deleted: false,
                    }],
                )
                .unwrap();
            writer.activate(Some("account")).unwrap();
        }
        assert_eq!(read().len(), 2);
        store.lock().unwrap().activate(None).unwrap();
        assert_eq!(read().len(), 1);
        assert!(query_shared(&store, "DELETE FROM observations").is_err());
        assert_eq!(read().len(), 1);
    }

    #[test]
    fn classification_facts_preserve_usage_and_do_not_cross_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("test.duckdb")).unwrap();
        let mut usage = event();
        usage.agent = "codex".into();
        usage.session_id = Some("session".into());
        usage.client = Some("codex_desktop".into());
        store.ingest(&usage, None).unwrap();
        let other = Event { event_id: "other".into(), host_id: "other-host".into(), ..usage.clone() };
        store.ingest(&other, None).unwrap();
        let fact = Event { event_id: "classification".into(), host_id: "host".into(),
            session_id: Some("session".into()), client: Some("chatgpt_desktop_cowork".into()),
            agent: "codex".into(), source: "log".into(), event_type: "session.client".into(),
            correlation_confidence: Some("observed_originator".into()), ..Default::default() };
        store.ingest(&fact, None).unwrap();
        store.ingest(&fact, None).unwrap();
        let rows = store.query("SELECT host_id,client,input_tokens,output_tokens FROM resolved_events ORDER BY host_id").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["client"], "chatgpt_desktop_cowork");
        assert_eq!(rows[1]["client"], "codex_desktop");
        for row in rows { assert_eq!(row["input_tokens"], 2); assert_eq!(row["output_tokens"], 16); }
    }
    fn event() -> Event {
        Event {
            event_id: "event-1".into(),
            host_id: "host".into(),
            event_type: "api.request".into(),
            model: Some("haiku".into()),
            input_tokens: Some(2),
            output_tokens: Some(16),
            ..Default::default()
        }
    }
    #[test]
    fn predecessor_is_counted_once_only_in_its_account() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut store = Store::open(&dir.path().join("events.duckdb"))?;
        let local = event();
        let remote = Event { event_id: "old-event".into(), host_id: "old-host".into(), ..local.clone() };
        store.ingest(&local, Some(("destination", &local)))?;
        store.connection.execute("INSERT INTO device_event_aliases VALUES (?,?,?,?)",
            params!["account", "destination", local.event_id, remote.event_id])?;
        assert!(store.acknowledge_predecessors("account", "destination").is_err());
        assert_eq!(store.pending_count("destination")?, 1);
        store.apply_page("account", 0, 1, &[Change {
            sequence: 1, event: remote, deleted: false,
        }])?;
        store.acknowledge_predecessors("account", "destination")?;
        assert_eq!(store.pending_count("destination")?, 0);
        assert_eq!(store.enqueue_history("destination", |_| true)?, 0);
        assert_eq!(store.enqueue_history("other", |_| true)?, 1);
        store.activate(Some("account"))?;
        let rows = store.query("SELECT event_id FROM raw_events")?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["event_id"], "old-event");
        store.activate(None)?;
        assert_eq!(store.query("SELECT event_id FROM raw_events")?[0]["event_id"], local.event_id);
        Ok(())
    }

    #[test]
    fn crash_replay_ack_and_workspace_isolation() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("events.duckdb");
        let e = event();
        {
            let mut s = Store::open(&path)?;
            s.ingest(&e, Some(("workspace-a", &e)))?;
        }
        let mut s = Store::open(&path)?;
        assert_eq!(s.pending("workspace-a", 500)?.len(), 1);
        assert!(s.pending("workspace-b", 500)?.is_empty());
        s.ingest(&e, Some(("workspace-b", &e)))?;
        assert!(s.pending("workspace-b", 500)?.is_empty());
        s.ingest(&e, Some(("workspace-a", &e)))?;
        assert_eq!(s.pending("workspace-a", 500)?.len(), 1);
        s.acknowledge("workspace-a", &[e.event_id.clone()])?;
        s.ingest(&e, Some(("workspace-a", &e)))?;
        assert!(s.pending("workspace-a", 500)?.is_empty());
        let mut conflict = e.clone();
        conflict.output_tokens = Some(99);
        assert!(s.ingest(&conflict, None).is_err());
        Ok(())
    }
    #[test]
    fn resolved_usage_links_are_shared_and_never_change_measured_usage() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut s = Store::open(&dir.path().join("events.duckdb"))?;
        let mut e = event();
        e.client = Some("claude_desktop".into());
        e.session_id = Some("plugin-session".into());
        e.correlation_key = Some("response".into());
        s.ingest(&e, None)?;
        let link = Event {
            event_id: "link".into(),
            host_id: e.host_id.clone(),
            event_type: "message.usage.link".into(),
            session_id: Some("chat".into()),
            parent_session_id: e.session_id.clone(),
            turn_id: Some("assistant".into()),
            correlation_key: e.correlation_key.clone(),
            correlation_confidence: Some("content_match".into()),
            ..Default::default()
        };
        s.ingest(&link, None)?;
        let rows =
            s.query("SELECT session_id,turn_id,input_tokens,output_tokens FROM resolved_events")?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["session_id"], "chat");
        assert_eq!(rows[0]["output_tokens"], 16);
        let reject = Event {
            event_id: "reject".into(),
            host_id: e.host_id.clone(),
            event_type: "message.usage.link.rejected".into(),
            session_id: e.session_id.clone(),
            correlation_key: e.correlation_key.clone(),
            ..Default::default()
        };
        s.ingest(&reject, None)?;
        let rows = s.query("SELECT session_id,input_tokens,output_tokens FROM resolved_events")?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["session_id"], "plugin-session");
        assert_eq!(rows[0]["input_tokens"], 2);
        Ok(())
    }

    #[test]
    fn invalid_cloud_payload_rolls_back_local_insert() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut s = Store::open(&dir.path().join("events.duckdb"))?;
        let mut e = event();
        e.prompt_text = Some("private".into());
        assert!(s.ingest(&e, Some(("a", &e))).is_err());
        let n: i64 = s
            .connection
            .query_row("SELECT count(*) FROM observations", [], |r| r.get(0))?;
        assert_eq!(n, 0);
        Ok(())
    }
    #[test]
    fn remote_pages_are_atomic_and_deleted_records_do_not_resurrect() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut s = Store::open(&dir.path().join("events.duckdb"))?;
        let change = Change {
            sequence: 1,
            event: event(),
            deleted: false,
        };
        s.apply_page("remote-a", 0, 1, &[change.clone()])?;
        assert!(s.apply_page("remote-a", 0, 1, &[change.clone()]).is_err());
        s.apply_page(
            "remote-a",
            1,
            2,
            &[Change {
                sequence: 2,
                deleted: true,
                ..change.clone()
            }],
        )?;
        s.apply_page(
            "remote-a",
            2,
            3,
            &[Change {
                sequence: 3,
                ..change
            }],
        )?;
        assert_eq!(s.cursor("remote-a")?, 3);
        let n: i64 = s
            .connection
            .query_row("SELECT count(*) FROM observations", [], |r| r.get(0))?;
        assert_eq!(n, 0);
        assert!(s.pending("remote-a", 500)?.is_empty());
        Ok(())
    }
}

#[cfg(test)]
mod cutover_tests {
    use super::*;
    #[test]
    #[ignore = "requires explicit KIKIMIMI_VERIFY_HISTORY; reads a snapshot of real history"]
    fn real_history_import_preserves_haiku_and_sonnet_usage() {
        let source = std::env::var("KIKIMIMI_VERIFY_HISTORY").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.duckdb");
        let mut store = Store::open(&path).unwrap();
        store.import_history(Path::new(&source)).unwrap();
        let sql="SELECT model,input_tokens,output_tokens FROM resolved_events WHERE event_type='api.request' AND session_id='0caac1cf-17d8-4552-8a66-d718adf6292c' ORDER BY ts";
        let rows = store.query(sql).unwrap();
        assert!(
            rows.iter()
                .any(|r| r["model"].as_str().is_some_and(|m| m.contains("haiku"))
                    && r["input_tokens"] == 2
                    && r["output_tokens"] == 16),
            "Haiku measurement missing"
        );
        assert!(
            rows.iter().any(
                |r| r["model"].as_str().is_some_and(|m| m.contains("sonnet"))
                    && r["input_tokens"] == 2
                    && r["output_tokens"] == 15
            ),
            "Sonnet measurement missing"
        );
        let count = store.query("SELECT count(*) n FROM raw_events").unwrap();
        store.import_history(Path::new(&source)).unwrap();
        assert_eq!(
            count,
            store.query("SELECT count(*) n FROM raw_events").unwrap()
        );
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(rows, store.query(sql).unwrap());
        println!(
            "import and restart: {} events; Haiku 2/16 and Sonnet 2/15 preserved",
            count[0]["n"]
        );
    }
}
