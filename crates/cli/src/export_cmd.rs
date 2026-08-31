//! `guru export [--from DT --to DT] [-o FILE]` — architecture.md §6 「エクスポート (pull)」,
//! §8 cloud API 契約 (`GET /v1/export`)。
//!
//! `guru.v1` Parquet 全量を Bearer 認証つきでストリーム取得し、ファイルへ書き出す。
//! これはロックイン回避の要 (§6: 「解約時の持ち出し」) なので Stage 0 から用意する。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;

const DEFAULT_OUTPUT: &str = "guru-export.parquet";

pub struct ExportArgs {
    pub dt_from: Option<String>,
    pub dt_to: Option<String>,
    pub output: Option<PathBuf>,
}

pub fn run(args: ExportArgs) -> anyhow::Result<()> {
    let cfg = crate::config::GuruConfig::load();
    let cloud = cfg
        .cloud
        .ok_or_else(|| anyhow::anyhow!("not logged in; run `guru login` first"))?;

    let url = export_url(
        &cloud.endpoint,
        args.dt_from.as_deref(),
        args.dt_to.as_deref(),
    );
    let output = args.output.unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT));

    let client = reqwest::blocking::Client::builder()
        // A full org export can be large; §6 puts no Stage-0 size cap on it, so this is
        // deliberately much longer than the 10s used for the small /v1/events pushes.
        .timeout(Duration::from_secs(300))
        .build()
        .context("building HTTP client")?;

    let mut resp = client
        .get(&url)
        .bearer_auth(&cloud.token)
        .send()
        .context("GET /v1/export")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        anyhow::bail!("GET /v1/export returned {status}: {body}");
    }

    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut file =
        std::fs::File::create(&output).with_context(|| format!("creating {}", output.display()))?;
    let bytes_written =
        std::io::copy(&mut resp, &mut file).context("streaming export response to file")?;
    file.sync_all().ok();

    print_summary(&output, bytes_written);
    Ok(())
}

fn export_url(endpoint: &str, dt_from: Option<&str>, dt_to: Option<&str>) -> String {
    let endpoint = endpoint.trim_end_matches('/');
    let mut params = Vec::new();
    if let Some(from) = dt_from {
        params.push(format!("dt_from={from}"));
    }
    if let Some(to) = dt_to {
        params.push(format!("dt_to={to}"));
    }
    if params.is_empty() {
        format!("{endpoint}/v1/export")
    } else {
        format!("{endpoint}/v1/export?{}", params.join("&"))
    }
}

/// row-group/行数のサマリを出す。壊れた/読めない Parquet だった場合でも (ネットワーク越し
/// なので万一途中で切れた等) ダウンロード自体は成功しているので、ここは失敗させずファイル
/// サイズだけの要約に落とす。
fn print_summary(path: &Path, file_size: u64) {
    match read_parquet_summary(path) {
        Ok((row_groups, rows)) => println!(
            "wrote {} ({}, {row_groups} row group(s), {rows} row(s))",
            path.display(),
            crate::status_cmd::human_bytes(file_size)
        ),
        Err(e) => {
            eprintln!(
                "warning: could not read parquet metadata from {}: {e:#}",
                path.display()
            );
            println!(
                "wrote {} ({})",
                path.display(),
                crate::status_cmd::human_bytes(file_size)
            );
        }
    }
}

fn read_parquet_summary(path: &Path) -> anyhow::Result<(usize, i64)> {
    use parquet::file::reader::FileReader;
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = parquet::file::serialized_reader::SerializedFileReader::new(file)
        .context("reading parquet footer")?;
    let metadata = reader.metadata();
    Ok((
        metadata.num_row_groups(),
        metadata.file_metadata().num_rows(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CloudConfig, GuruConfig};
    use httpmock::prelude::*;
    use serial_test::serial;

    fn login_with(server: &MockServer) {
        let mut cfg = GuruConfig::load();
        cfg.cloud = Some(CloudConfig {
            endpoint: server.base_url(),
            token: "tok-export".into(),
            email: "dev@example.com".into(),
            org_id: "org-1".into(),
        });
        cfg.save().unwrap();
    }

    #[test]
    #[serial]
    fn export_errors_when_not_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("GURU_DIR", dir.path());

        let out = dir.path().join("out.parquet");
        let result = run(ExportArgs {
            dt_from: None,
            dt_to: None,
            output: Some(out),
        });
        assert!(result.is_err());

        std::env::remove_var("GURU_DIR");
    }

    #[test]
    #[serial]
    fn export_streams_body_with_bearer_auth_to_default_output() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("GURU_DIR", dir.path());
        // Run inside the tempdir so the *default* output filename lands somewhere we clean up.
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let server = MockServer::start();
        login_with(&server);
        let payload = b"fake-parquet-bytes-not-a-real-file".to_vec();
        let payload2 = payload.clone();
        let mock = server.mock(move |when, then| {
            when.method(GET)
                .path("/v1/export")
                .header("authorization", "Bearer tok-export");
            then.status(200)
                .header("content-type", "application/vnd.apache.parquet")
                .body(payload2.clone());
        });

        run(ExportArgs {
            dt_from: None,
            dt_to: None,
            output: None,
        })
        .unwrap();

        mock.assert_calls(1);
        let written = std::fs::read(DEFAULT_OUTPUT).unwrap();
        assert_eq!(written, payload);

        std::env::set_current_dir(prev_cwd).unwrap();
        std::env::remove_var("GURU_DIR");
    }

    #[test]
    #[serial]
    fn export_includes_dt_range_in_query_string_and_writes_custom_output() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("GURU_DIR", dir.path());

        let server = MockServer::start();
        login_with(&server);
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/export")
                .query_param("dt_from", "2026-08-01")
                .query_param("dt_to", "2026-08-31");
            then.status(200).body(b"parquet-bytes".to_vec());
        });

        let out = dir.path().join("custom.parquet");
        run(ExportArgs {
            dt_from: Some("2026-08-01".into()),
            dt_to: Some("2026-08-31".into()),
            output: Some(out.clone()),
        })
        .unwrap();

        mock.assert_calls(1);
        assert_eq!(std::fs::read(&out).unwrap(), b"parquet-bytes");

        std::env::remove_var("GURU_DIR");
    }

    #[test]
    #[serial]
    fn export_returns_error_on_non_success_status() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("GURU_DIR", dir.path());

        let server = MockServer::start();
        login_with(&server);
        server.mock(|when, then| {
            when.method(GET).path("/v1/export");
            then.status(401).body("unauthorized");
        });

        let out = dir.path().join("out.parquet");
        let result = run(ExportArgs {
            dt_from: None,
            dt_to: None,
            output: Some(out.clone()),
        });
        assert!(result.is_err());
        assert!(!out.exists(), "must not create the output file on failure");

        std::env::remove_var("GURU_DIR");
    }

    #[test]
    fn export_url_builds_query_string_only_when_range_given() {
        assert_eq!(export_url("http://x", None, None), "http://x/v1/export");
        assert_eq!(
            export_url("http://x/", Some("2026-08-01"), None),
            "http://x/v1/export?dt_from=2026-08-01"
        );
        assert_eq!(
            export_url("http://x", Some("2026-08-01"), Some("2026-08-31")),
            "http://x/v1/export?dt_from=2026-08-01&dt_to=2026-08-31"
        );
    }
}
