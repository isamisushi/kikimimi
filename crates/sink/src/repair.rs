//! Local history repair. Caller must hold the collector's exclusive control socket.
use crate::{EventSink, FileSink};
use anyhow::Context;
use arrow_array::{Array, StringArray};
use kikimimi_schema::Event;
use parquet::arrow::{arrow_reader::ParquetRecordBatchReaderBuilder, ProjectionMask};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// Append only absent event IDs. Scan failures abort before writing anything.
/// A retry after a partially completed flush discovers the committed partitions,
/// so neither a crash nor a missing migration marker creates duplicate rows.
pub fn append_missing_events(
    data_dir: &Path,
    host_id: &str,
    events: Vec<Event>,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut missing: HashMap<String, Event> = events
        .into_iter()
        .map(|e| (e.event_id.clone(), e))
        .collect();
    if missing.is_empty() {
        return Ok(vec![]);
    }
    if data_dir.exists() {
        for partition in fs::read_dir(data_dir)? {
            let partition = partition?;
            if !partition.file_type()?.is_dir()
                || !partition.file_name().to_string_lossy().starts_with("dt=")
            {
                continue;
            }
            for file in fs::read_dir(partition.path())? {
                let path = file?.path();
                if path.extension().and_then(|s| s.to_str()) != Some("parquet") {
                    continue;
                }
                let builder = ParquetRecordBatchReaderBuilder::try_new(fs::File::open(&path)?)
                    .with_context(|| format!("reading event IDs from {}", path.display()))?;
                let index = builder.schema().index_of("event_id")?;
                let projection = ProjectionMask::roots(builder.parquet_schema(), [index]);
                for batch in builder.with_projection(projection).build()? {
                    let batch = batch?;
                    let ids = batch
                        .column(0)
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .context("event_id must be a string column")?;
                    for i in 0..ids.len() {
                        if !ids.is_null(i) {
                            missing.remove(ids.value(i));
                        }
                    }
                }
            }
        }
    }
    let mut sink = FileSink::new(
        data_dir.to_path_buf(),
        host_id.to_owned(),
        500,
        Duration::from_secs(30),
    );
    for event in missing.into_values() {
        sink.push(event);
    }
    sink.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repair_is_idempotent_including_partial_previous_writes() {
        let d = tempfile::tempdir().unwrap();
        let event = |id: &str| Event {
            event_id: id.into(),
            dt: "2026-10-29".into(),
            ..Default::default()
        };
        assert_eq!(
            append_missing_events(d.path(), "host", vec![event("a")])
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            append_missing_events(d.path(), "host", vec![event("a"), event("b"), event("b")])
                .unwrap()
                .len(),
            1
        );
        assert!(
            append_missing_events(d.path(), "host", vec![event("a"), event("b")])
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn unreadable_history_fails_closed() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("dt=2026-10-29")).unwrap();
        fs::write(d.path().join("dt=2026-10-29/bad.parquet"), b"broken").unwrap();
        assert!(append_missing_events(
            d.path(),
            "host",
            vec![Event {
                event_id: "a".into(),
                ..Default::default()
            }]
        )
        .is_err());
    }
}

/// Rekey old Codex metadata and remove only identical records. All input is read
/// and validated before mutation. Each replacement is atomic and has a backup;
/// IDs are content-derived so retrying after any committed prefix is safe.
/// Caller must hold the collector lock. Unknown Arrow columns are preserved.
pub fn repair_metadata_history(data_dir: &Path) -> anyhow::Result<(usize, usize)> {
    use arrow_array::{ArrayRef, RecordBatch};
    use parquet::arrow::ArrowWriter;
    use std::{collections::HashMap, sync::Arc};
    let mut paths = vec![];
    if !data_dir.exists() {
        return Ok((0, 0));
    }
    for dir in fs::read_dir(data_dir)? {
        let dir = dir?;
        if !dir.file_type()?.is_dir() || !dir.file_name().to_string_lossy().starts_with("dt=") {
            continue;
        }
        for file in fs::read_dir(dir.path())? {
            let path = file?.path();
            if path.extension().and_then(|x| x.to_str()) == Some("parquet") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    let mut seen = HashMap::new();
    let mut planned = vec![];
    let mut removed = 0;
    for path in paths {
        let builder = ParquetRecordBatchReaderBuilder::try_new(fs::File::open(&path)?)?;
        let schema = builder.schema().clone();
        let index = schema.index_of("event_id")?;
        let mut rows = vec![];
        let mut changed = false;
        for batch in builder.build()? {
            let batch = batch?;
            for i in 0..batch.num_rows() {
                let value = row_json(&batch, i)?;
                let event: Event = serde_json::from_value(value.clone())?;
                if !is_log_metadata(&event) {
                    rows.push(batch.slice(i, 1));
                    continue;
                }
                let id = kikimimi_schema::codex_metadata_id(&event)
                    .unwrap_or_else(|| event.event_id.clone());
                // Include unknown columns in the equality check: never silently
                // erase metadata written by a newer schema.
                let mut canonical = value;
                canonical.as_object_mut().unwrap().remove("event_id");
                let fingerprint = serde_json::to_string(&canonical)?;
                if let Some(previous) = seen.insert(id.clone(), fingerprint.clone()) {
                    anyhow::ensure!(
                        previous == fingerprint,
                        "metadata ID collision with differing content: {id}"
                    );
                    changed = true;
                    removed += 1;
                    continue;
                }
                changed |= id != event.event_id;
                let mut columns = batch.slice(i, 1).columns().to_vec();
                columns[index] = Arc::new(StringArray::from(vec![id])) as ArrayRef;
                rows.push(RecordBatch::try_new(schema.clone(), columns)?);
            }
        }
        if changed {
            planned.push((path, schema, rows));
        }
    }
    let files = planned.len();
    for (path, schema, rows) in planned {
        // Retained beside the original, outside the *.parquet reader glob.
        let backup = path.with_extension("parquet.before-metadata-v2");
        if !backup.exists() {
            fs::copy(&path, &backup)?;
            fs::File::open(&backup)?.sync_all()?;
        }
        let tmp = path.with_extension("parquet.metadata-v2.tmp");
        let file = fs::File::create(&tmp)?;
        let props = parquet::file::properties::WriterProperties::builder()
            .set_compression(parquet::basic::Compression::ZSTD(Default::default()))
            .build();
        let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
        for row in rows {
            writer.write(&row)?;
        }
        writer.close()?;
        fs::File::open(&tmp)?.sync_all()?;
        fs::rename(&tmp, &path)?;
        fs::File::open(path.parent().unwrap())?.sync_all()?;
    }
    Ok((files, removed))
}

fn row_json(batch: &arrow_array::RecordBatch, row: usize) -> anyhow::Result<serde_json::Value> {
    use arrow_array::{BooleanArray, Float64Array, Int64Array};
    use arrow_schema::DataType;
    let mut values = serde_json::Map::new();
    for (field, column) in batch.schema().fields().iter().zip(batch.columns()) {
        let value = if column.is_null(row) {
            serde_json::Value::Null
        } else {
            match field.data_type() {
                DataType::Utf8 => serde_json::json!(column
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap()
                    .value(row)),
                DataType::Int64 => serde_json::json!(column
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap()
                    .value(row)),
                DataType::Float64 => serde_json::json!(column
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap()
                    .value(row)),
                DataType::Boolean => serde_json::json!(column
                    .as_any()
                    .downcast_ref::<BooleanArray>()
                    .unwrap()
                    .value(row)),
                other => anyhow::bail!("unsupported history column {}: {other:?}", field.name()),
            }
        };
        values.insert(field.name().clone(), value);
    }
    Ok(serde_json::Value::Object(values))
}

/// Drop replays of metadata already committed in this date partition.
pub(crate) fn without_metadata_replays(dir: &Path, events: &[Event]) -> anyhow::Result<Vec<Event>> {
    let mut ids = std::collections::HashSet::new();
    if events.iter().any(|e| is_log_metadata(e)) && dir.exists() {
        for file in fs::read_dir(dir)? {
            let path = file?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("parquet") {
                continue;
            }
            let builder = ParquetRecordBatchReaderBuilder::try_new(fs::File::open(path)?)?;
            let index = builder.schema().index_of("event_id")?;
            let mask = ProjectionMask::roots(builder.parquet_schema(), [index]);
            for batch in builder.with_projection(mask).build()? {
                let batch = batch?;
                let values = batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .context("event_id must be Utf8")?;
                for value in values.iter().flatten() {
                    ids.insert(value.to_owned());
                }
            }
        }
    }
    Ok(events
        .iter()
        .filter(|e| !is_log_metadata(e) || ids.insert(e.event_id.clone()))
        .cloned()
        .collect())
}

#[cfg(test)]
mod metadata_tests {
    use super::*;
    fn event(ts: i64, tokens: i64) -> Event {
        Event {
            event_id: "colliding-old-id".into(),
            agent: "codex".into(),
            source: "log".into(),
            event_type: "api.request".into(),
            ts,
            dt: "2026-10-29".into(),
            host_id: "host".into(),
            session_id: Some("session".into()),
            input_tokens: Some(tokens),
            ..Default::default()
        }
    }
    fn read(dir: &Path) -> Vec<Event> {
        let mut events = vec![];
        for file in fs::read_dir(dir.join("dt=2026-10-29")).unwrap() {
            let path = file.unwrap().path();
            if path.extension().and_then(|s| s.to_str()) != Some("parquet") {
                continue;
            }
            for batch in ParquetRecordBatchReaderBuilder::try_new(fs::File::open(path).unwrap())
                .unwrap()
                .build()
                .unwrap()
            {
                let batch = batch.unwrap();
                for i in 0..batch.num_rows() {
                    events.push(serde_json::from_value(row_json(&batch, i).unwrap()).unwrap());
                }
            }
        }
        events
    }
    #[test]
    fn repair_preserves_collisions_and_only_removes_exact_duplicates() {
        let d = tempfile::tempdir().unwrap();
        let partition = d.path().join("dt=2026-10-29");
        let a = event(1, 10);
        let b = event(2, 20);
        let c = event(1, 30);
        crate::write_parquet_partition(&partition, "host", 0, &[a.clone(), b.clone()]).unwrap();
        crate::write_parquet_partition(&partition, "host", 1, &[a, c.clone()]).unwrap();
        assert_eq!(repair_metadata_history(d.path()).unwrap(), (2, 1));
        let rows = read(d.path());
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows.iter().map(|e| e.input_tokens.unwrap()).sum::<i64>(),
            60
        );
        assert_eq!(
            rows.iter()
                .map(|e| &e.event_id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
        assert_eq!(repair_metadata_history(d.path()).unwrap(), (0, 0));
        // Simulate another old partition left after a partially committed repair.
        crate::write_parquet_partition(&partition, "host", 2, &[b, c]).unwrap();
        assert_eq!(repair_metadata_history(d.path()).unwrap().1, 2);
        assert_eq!(read(d.path()).len(), 3);
    }
    #[test]
    fn live_replays_are_deduplicated_across_flush_and_restart() {
        let d = tempfile::tempdir().unwrap();
        let mut e = event(1, 10);
        e.event_id = kikimimi_schema::codex_metadata_id(&e).unwrap();
        for _ in 0..2 {
            let mut sink =
                FileSink::new(d.path().into(), "host".into(), 500, Duration::from_secs(30));
            sink.push(e.clone());
            sink.push(e.clone());
            sink.flush().unwrap();
        }
        assert_eq!(read(d.path()).len(), 1);
    }
    #[test]
    fn corrupt_history_aborts_before_modifying_valid_files() {
        let d = tempfile::tempdir().unwrap();
        let partition = d.path().join("dt=2026-10-29");
        let path = crate::write_parquet_partition(&partition, "host", 0, &[event(1, 10)]).unwrap();
        let before = fs::read(&path).unwrap();
        fs::write(partition.join("zzz.parquet"), b"broken").unwrap();
        assert!(repair_metadata_history(d.path()).is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

fn is_log_metadata(e: &Event) -> bool {
    e.source == "log"
        && matches!(
            e.event_type.as_str(),
            "session.start" | "turn" | "api.request"
        )
}
