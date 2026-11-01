#!/usr/bin/python3
"""Build this Mac's app and install it atomically, preserving collection state."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
import uuid

DESKTOP = Path(__file__).resolve().parents[1]
ROOT = DESKTOP.parent
CACHE = DESKTOP / ".cache"
INSTALLED = Path("/Applications/kikimimi.app")
DUCKDB = {
    "aarch64-apple-darwin": ("arm64", "188a36fe8883ef52e69165a0cbb6031ef679c8e6c448a1ce22951964149b7f96"),
    "x86_64-apple-darwin": ("amd64", "fbc7013ba09d117396dabf16783281c2add47fc01cdac4855464a03526dc96ee"),
}


def run(args, env, capture=False, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, env=env, text=True) if capture else subprocess.run(args, cwd=cwd, env=env, check=True)


def pinned_duckdb(host, env):
    arch, expected = DUCKDB[host]
    directory = CACHE / "duckdb" / "2.0.0" / arch
    directory.mkdir(parents=True, exist_ok=True)
    archive = directory / "duckdb.tar.gz"
    if not archive.exists() or hashlib.sha256(archive.read_bytes()).hexdigest() != expected:
        download = directory / "duckdb.download"
        url = f"https://github.com/duckdb/duckdb/releases/download/v2.0.0/duckdb-cli-osx-{arch}.tar.gz"
        try:
            run(["curl", "--fail", "--location", "--retry", "3", "--output", str(download), url], env)
            if hashlib.sha256(download.read_bytes()).hexdigest() != expected:
                raise RuntimeError("DuckDB checksum does not match the official release")
            download.replace(archive)
        finally:
            download.unlink(missing_ok=True)
    binary = directory / "duckdb"
    with tarfile.open(archive) as package:
        member = package.getmember("duckdb")
        if not member.isfile():
            raise RuntimeError("Expected a regular DuckDB executable")
        with package.extractfile(member) as source, binary.open("wb") as target:
            shutil.copyfileobj(source, target)
    binary.chmod(0o755)
    return binary


def collection_status(app, env):
    result = run([str(app / "Contents/MacOS/kikimimi"), "desktop", "status"], env, capture=True)
    return json.loads(result)


def swap(left, right):
    libc = ctypes.CDLL(None, use_errno=True)
    rename = libc.renamex_np
    rename.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
    rename.restype = ctypes.c_int
    if rename(os.fsencode(left), os.fsencode(right), 2):
        raise OSError(ctypes.get_errno(), "Atomic application swap failed")


def close_installed_app(env):
    executable = str(INSTALLED / "Contents/MacOS/kikimimi-desktop")
    processes = run(["ps", "-axo", "pid=,comm="], env, capture=True)
    pids = [int(parts[0]) for line in processes.splitlines()
            if len(parts := line.strip().split(None, 1)) == 2 and parts[1] == executable]
    for pid in pids:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    deadline = time.monotonic() + 10
    for pid in pids:
        while True:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError("Quit kikimimi before retrying; the installed app has not been replaced")
            time.sleep(0.1)
    return bool(pids)


def install(app, env):
    if INSTALLED.is_symlink():
        raise RuntimeError("The installed application must not be a symbolic link")
    had_previous = INSTALLED.exists()
    if had_previous:
        previous = json.loads(run(["/usr/bin/plutil", "-convert", "json", "-o", "-", str(INSTALLED / "Contents/Info.plist")], env, capture=True))
        if previous.get("CFBundleIdentifier") != "dev.kikimimi.desktop":
            raise RuntimeError("Refusing to replace an unrelated application")
    before = collection_status(INSTALLED if had_previous else app, env)
    stage = Path(tempfile.mkdtemp(prefix=".kikimimi-local-", dir="/Applications"))
    incoming = stage / "kikimimi.app"
    try:
        run(["ditto", str(app), str(incoming)], env)
        run(["codesign", "--verify", "--deep", "--strict", str(incoming)], env)
        was_open = close_installed_app(env)
        if had_previous:
            swap(incoming, INSTALLED)
        else:
            incoming.rename(INSTALLED)
        try:
            if before["running"]:
                run([str(INSTALLED / "Contents/MacOS/kikimimi"), "desktop", "restart-after-update"], env)
            after = collection_status(INSTALLED, env)
            if after["running"] != before["running"]:
                raise RuntimeError("Collection did not retain its previous running state")
        except BaseException:
            if had_previous:
                swap(incoming, INSTALLED)
                if before["running"]:
                    run([str(INSTALLED / "Contents/MacOS/kikimimi"), "desktop", "restart-after-update"], env)
                if was_open:
                    run(["open", "-a", str(INSTALLED)], env)
            raise
        if had_previous:
            backup = CACHE / "local-backups" / (time.strftime("%Y%m%d-%H%M%S") + "-" + uuid.uuid4().hex[:6]) / "kikimimi.app"
            backup.parent.mkdir(parents=True)
            shutil.move(str(incoming), str(backup))
            print(f"Previous app: {backup}", flush=True)
        if was_open:
            run(["open", "-a", str(INSTALLED)], env)
        print(f"Installed: {INSTALLED}; collection {'running' if after['running'] else 'stopped'}", flush=True)
    finally:
        # Preserve any previous app left in staging if copying its backup failed.
        if incoming.exists():
            print(f"Retained staged app: {incoming}", file=sys.stderr)
        else:
            stage.rmdir()


def main():
    started = time.monotonic()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-only", action="store_true", help="Build without installing")
    args = parser.parse_args()
    if sys.platform != "darwin" or os.geteuid() == 0:
        raise RuntimeError("Run this command as your normal user on a Mac, without sudo")
    env = os.environ.copy()
    env["PATH"] = str(Path.home() / ".cargo/bin") + os.pathsep + env.get("PATH", "")
    for name in ["APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID", "APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD", "APPLE_API_KEY", "APPLE_API_ISSUER", "APPLE_API_KEY_PATH", "TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PATH", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "TAURI_CONFIG", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET", "TAURI_ENV_TARGET_TRIPLE"]:
        env.pop(name, None)
    env.update(APPLE_SIGNING_IDENTITY="-", KIKIMIMI_UPDATER_PUBLIC_KEY="")
    if not shutil.which("npm", path=env["PATH"]) or not shutil.which("cargo", path=env["PATH"]):
        raise RuntimeError("Node/npm and Rust are required")
    host = run(["rustc", "--print", "host-tuple"], env, capture=True).strip()
    if host not in DUCKDB:
        raise RuntimeError(f"Unsupported Mac target: {host}")
    if not any(path.exists() for path in [DESKTOP / "node_modules/.bin/tauri", ROOT / "node_modules/.bin/tauri"]):
        run(["npm", "ci"], env)
    env["DUCKDB_BINARY"] = str(pinned_duckdb(host, env))
    print(f"Building for this Mac ({host}) with local signing…", flush=True)
    run(["npm", "run", "build", "--", "--bundles", "app"], env, cwd=DESKTOP)
    app = DESKTOP / "src-tauri/target/release/bundle/macos/kikimimi.app"
    info = json.loads(run(["/usr/bin/plutil", "-convert", "json", "-o", "-", str(app / "Contents/Info.plist")], env, capture=True))
    config = json.loads((DESKTOP / "src-tauri/tauri.conf.json").read_text())
    if info.get("CFBundleIdentifier") != config["identifier"] or info.get("CFBundleShortVersionString") != config["version"]:
        raise RuntimeError("Built application identity/version does not match the project")
    run(["codesign", "--verify", "--deep", "--strict", str(app)], env)
    if not args.build_only:
        install(app, env)
    print(f"Local build {config['version']} complete in {time.monotonic() - started:.1f}s", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Local build failed: {error}", file=sys.stderr)
        sys.exit(1)
