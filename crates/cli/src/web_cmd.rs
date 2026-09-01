//! `kikimimi web` — prints the local web UI URL (architecture.md §8) and makes a
//! best-effort attempt to open it in a browser. Same URL `kikimimi status`
//! prints; this is just the one-liner for "open it for me".

pub fn run() -> anyhow::Result<()> {
    if !kikimimi_spool::send_control(b'n') {
        anyhow::bail!("kikimimi agent is not running; start it with `kikimimi agent` first");
    }

    let state = crate::state::load_opt(&kikimimi_schema::paths::state_path()).ok_or_else(|| {
        anyhow::anyhow!(
            "state.json not found or unreadable; is `kikimimi agent` still starting up?"
        )
    })?;

    if let Some(err) = &state.web_error {
        anyhow::bail!("kikimimi agent's web UI failed to start: {err}");
    }
    if state.web.port == 0 {
        anyhow::bail!(
            "no web UI port recorded yet in state.json; is `kikimimi agent` still starting up?"
        );
    }

    let url = format!("http://127.0.0.1:{}/?t={}", state.web.port, state.web.token);
    println!("{url}");
    try_open_browser(&url);
    Ok(())
}

/// Best-effort only (task spec: "ignore failures"): tries `$BROWSER <url>`
/// first, then `xdg-open <url>` (Linux desktop convention), then macOS's
/// `open <url>`. Never surfaces an error either way -- the URL is already
/// printed, which is the part that actually matters.
fn try_open_browser(url: &str) {
    if let Ok(browser) = std::env::var("BROWSER") {
        if !browser.is_empty() && spawn_detached(&browser, url) {
            return;
        }
    }
    if spawn_detached("xdg-open", url) {
        return;
    }
    spawn_detached("open", url);
}

fn spawn_detached(cmd: &str, url: &str) -> bool {
    std::process::Command::new(cmd)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}
