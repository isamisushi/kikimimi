#!/usr/bin/python3
"""Build/sign/notarize public Mac packages locally, using login Keychain."""
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

from signing_support import credentials, identity, ensure_notary_profile, ROOT, DESKTOP

spec = importlib.util.spec_from_file_location("local_install", DESKTOP / "scripts/install-local.py")
local = importlib.util.module_from_spec(spec)
spec.loader.exec_module(local)


def run(args, env, capture=False):
    if capture:
        return subprocess.check_output(args, cwd=ROOT, env=env, text=True)
    return subprocess.run(args, cwd=ROOT, env=env, check=True)


def retry(args, env):
    for attempt in range(3):
        result = subprocess.run(args, cwd=ROOT, env=env)
        if result.returncode == 0:
            return
        if attempt < 2:
            time.sleep(3)
    raise RuntimeError("Validation failed after three attempts")


def info(app, env):
    return json.loads(run(["plutil", "-convert", "json", "-o", "-", str(app / "Contents/Info.plist")], env, True))


def public_packet(values):
    lines = base64.b64decode(values["KIKIMIMI_UPDATER_PUBLIC_KEY"]).decode().strip().splitlines()
    if len(lines) != 2 or not lines[0].startswith("untrusted comment:"):
        raise RuntimeError("Invalid updater public key")
    return lines[1]


def verify_update(archive, signature, values, env, work):
    decoded = work / "archive.minisig"
    decoded.write_bytes(base64.b64decode(Path(signature).read_text().strip()))
    run(["minisign", "-V", "-P", public_packet(values), "-m", str(archive), "-x", str(decoded)], env)


def sign_update(archive, values, env):
    private_env = env.copy()
    for name in ["TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"]:
        private_env[name] = values.get(name, "")
    result = subprocess.run(["npm", "exec", "--workspace", "desktop", "--", "tauri", "signer", "sign", str(archive)],
                            cwd=ROOT, env=private_env, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("Updater signing failed; secret output suppressed")
    print("Updater archive signed with the existing release key", flush=True)


def notarize(file, receipt, profile, env):
    digest = hashlib.sha256(file.read_bytes()).hexdigest()
    saved = json.loads(receipt.read_text()) if receipt.exists() else {}
    if saved.get("sha256") == digest and saved.get("status") == "Accepted":
        print(f"Reusing accepted notarization for {file.name}", flush=True)
        return
    if saved.get("sha256") != digest or not saved.get("id"):
        result = subprocess.run(["xcrun", "notarytool", "submit", str(file), "--keychain-profile", profile,
                                 "--output-format", "json", "--no-progress"], env=env, capture_output=True, text=True)
        if result.returncode:
            raise RuntimeError("Apple notarization submission failed; private output suppressed")
        submitted = json.loads(result.stdout)
        saved = {"sha256": digest, "id": submitted["id"], "status": "In Progress"}
        receipt.write_text(json.dumps(saved, indent=2) + "\n")
    print(f"Waiting for Apple notarization: {file.name} ({saved['id']})", flush=True)
    deadline = time.monotonic() + 1200
    while time.monotonic() < deadline:
        result = subprocess.run(["xcrun", "notarytool", "info", saved["id"], "--keychain-profile", profile,
                                 "--output-format", "json"], env=env, capture_output=True, text=True, timeout=60)
        if result.returncode == 0:
            status = json.loads(result.stdout)["status"]
            saved["status"] = status
            receipt.write_text(json.dumps(saved, indent=2) + "\n")
            if status == "Accepted":
                print(f"Apple notarization accepted: {file.name}", flush=True)
                return
            if status not in ["In Progress", "Uploaded", "Submitted"]:
                raise RuntimeError(f"Apple notarization returned {status}; submission {saved['id']}")
        time.sleep(5)
    raise RuntimeError("Apple notarization is still pending; use --resume to continue without rebuilding")


def validate_app(app, arch, version, config, env):
    metadata = info(app, env)
    if metadata.get("CFBundleIdentifier") != config["identifier"] or metadata.get("CFBundleShortVersionString") != version:
        raise RuntimeError("Application identity/version does not match this release")
    for name in ["kikimimi-desktop", "kikimimi", "duckdb"]:
        binary = app / "Contents/MacOS" / name
        if not binary.is_file() or binary.is_symlink():
            raise RuntimeError("Expected a regular bundled executable")
        run(["lipo", str(binary), "-verify_arch", "arm64" if arch == "aarch64" else "x86_64"], env)
    run(["codesign", "--verify", "--deep", "--strict", str(app)], env)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=["aarch64", "x86_64"])
    sources = parser.add_mutually_exclusive_group()
    sources.add_argument("--reuse-archive", type=Path, help="Repackage a verified, previously built updater archive")
    sources.add_argument("--resume", action="store_true", help="Continue the saved package without rebuilding")
    parser.add_argument("--publish", action="store_true", help="Publish after both architectures are staged")
    parser.add_argument("--install", action="store_true", help="Install this Mac's signed app after validation")
    parser.add_argument("--setup-notary", action="store_true", help="Refresh the Keychain notarization profile")
    args = parser.parse_args()
    if sys.platform != "darwin" or os.geteuid() == 0:
        raise RuntimeError("Run as your normal user on a Mac, without sudo")
    started = time.monotonic()
    values = credentials()
    signer = identity(values)
    profile = ensure_notary_profile(values, signer["teamId"], args.setup_notary)
    env = {k: v for k, v in os.environ.items() if not k.startswith(("APPLE_", "TAURI_SIGNING_"))}
    env["PATH"] = str(Path.home() / ".cargo/bin") + os.pathsep + env.get("PATH", "")
    for name in ["TAURI_CONFIG", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET", "TAURI_ENV_TARGET_TRIPLE"]:
        env.pop(name, None)
    env.update(APPLE_SIGNING_IDENTITY=signer["identity"], KIKIMIMI_UPDATER_PUBLIC_KEY=values["KIKIMIMI_UPDATER_PUBLIC_KEY"])
    if not shutil.which("minisign", path=env["PATH"]):
        raise RuntimeError("minisign is required to verify updater signatures")
    host = run(["rustc", "--print", "host-tuple"], env, True).strip()
    native_arch = host.split("-")[0]
    arch = args.arch or native_arch
    if arch != native_arch and not (args.reuse_archive or args.resume):
        raise RuntimeError("Build natively, or provide a previously signed archive for the other architecture")
    if args.install and arch != native_arch:
        raise RuntimeError("Only this Mac's native app can be installed")
    config = json.loads((DESKTOP / "src-tauri/tauri.conf.json").read_text())
    version = config["version"]
    work = DESKTOP / ".cache/signed-release" / version / arch
    work.mkdir(parents=True, exist_ok=True)
    app = work / "kikimimi.app"
    bundle = work / "bundle"
    bundle.mkdir(exist_ok=True)
    if not args.resume:
        if app.exists():
            shutil.rmtree(app)
        if args.reuse_archive:
            archive = args.reuse_archive.resolve()
            verify_update(archive, str(archive) + ".sig", values, env, work)
            run(["tar", "-xzf", str(archive), "-C", str(work)], env)
            validate_app(app, arch, version, config, env)
        else:
            env["DUCKDB_BINARY"] = str(local.pinned_duckdb(host, env))
            print(f"Building signed {arch} app locally", flush=True)
            run(["npm", "run", "build", "--workspace", "desktop", "--", "--bundles", "app"], env)
            run(["ditto", str(DESKTOP / "src-tauri/target/release/bundle/macos/kikimimi.app"), str(app)], env)
        notices = app / "Contents/Resources/licenses"
        notices.mkdir(parents=True, exist_ok=True)
        shutil.copy2(DESKTOP / "licenses/LICENSE-DuckDB.txt", notices / "LICENSE-DuckDB.txt")
        shutil.copy2(ROOT / "LICENSE", notices / "LICENSE-kikimimi.txt")
        shutil.copy2(ROOT / "NOTICE", notices / "NOTICE-kikimimi.txt")
        run(["xattr", "-cr", str(app)], env)
        for target in [app / "Contents/MacOS" / name for name in ["kikimimi", "duckdb", "kikimimi-desktop"]] + [app]:
            run(["codesign", "--force", "--options", "runtime", "--timestamp", "--sign", signer["identity"], str(target)], env)
        run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(app), str(work / "notary-app.zip")], env)
    validate_app(app, arch, version, config, env)
    notarize(work / "notary-app.zip", work / "notary-app.json", profile, env)
    retry(["xcrun", "stapler", "staple", str(app)], env)
    retry(["xcrun", "stapler", "validate", str(app)], env)
    retry(["spctl", "--assess", "--type", "execute", str(app)], env)
    archive = bundle / "kikimimi.app.tar.gz"
    if not args.resume or not archive.exists():
        run(["tar", "-czf", str(archive), "-C", str(work), "kikimimi.app"], env)
    # Validate the actual updater payload after a tar round trip.
    with tempfile.TemporaryDirectory(prefix="kikimimi-updater-check-", dir=work) as directory:
        run(["tar", "-xzf", str(archive), "-C", directory], env)
        extracted = Path(directory) / "kikimimi.app"
        validate_app(extracted, arch, version, config, env)
        retry(["xcrun", "stapler", "validate", str(extracted)], env)
    sign_update(archive, values, env)
    dmg = bundle / f"kikimimi-{version}-{arch}.dmg"
    if not args.resume or not dmg.exists():
        with tempfile.TemporaryDirectory(prefix="kikimimi-dmg-", dir=work) as directory:
            run(["ditto", str(app), str(Path(directory) / "kikimimi.app")], env)
            (Path(directory) / "Applications").symlink_to("/Applications")
            # hdiutil's automatic estimate can underallocate for the Intel DuckDB binary.
            logical_bytes = sum(p.stat().st_size for p in app.rglob("*") if p.is_file())
            size_mb = max(256, (logical_bytes * 12 // 10) // (1024 * 1024) + 64)
            run(["hdiutil", "create", "-volname", "kikimimi", "-srcfolder", directory,
                 "-size", f"{size_mb}m", "-format", "UDZO", "-ov", str(dmg)], env)
        run(["codesign", "--force", "--timestamp", "--sign", signer["identity"], str(dmg)], env)
    notarize(dmg, work / "notary-dmg.json", profile, env)
    retry(["xcrun", "stapler", "staple", str(dmg)], env)
    retry(["xcrun", "stapler", "validate", str(dmg)], env)
    run(["codesign", "--verify", "--strict", str(dmg)], env)
    run(["node", "desktop/scripts/release.mjs", "stage", str(bundle), "desktop/release-artifacts", arch], env)
    if args.install:
        local.install(app, env)
    if args.publish:
        expected = {f"kikimimi-desktop-{version}-{platform}.{suffix}"
                    for platform in ["aarch64", "x86_64"]
                    for suffix in ["app.tar.gz", "app.tar.gz.sig", "dmg"]}
        actual = {file.name for file in (DESKTOP / "release-artifacts").iterdir()}
        if actual != expected:
            raise RuntimeError("Publish requires exactly the current version's six verified files for both architectures")
        dirty = run(["git", "status", "--porcelain", "--untracked-files=normal"], env, True)
        if any(line[3:] != ".serena/project.yml" for line in dirty.splitlines()):
            raise RuntimeError("Commit source changes before publishing")
        env["GITHUB_SHA"] = run(["git", "rev-parse", "HEAD"], env, True).strip()
        run(["node", "desktop/scripts/release.mjs", "manifest", "desktop/release-artifacts", "desktop/latest.json"], env)
        run(["node", "desktop/scripts/publish-release.mjs"], env)
    print(f"Signed and notarized {version} {arch} ready in {time.monotonic() - started:.1f}s", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        print(f"Local public release failed: {error}", file=sys.stderr)
        sys.exit(1)
