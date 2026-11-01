"""Private credential pipes and local notarization-profile setup."""
import json
import os
from pathlib import Path
import pty
import select
import signal
import subprocess
import termios
import time

DESKTOP = Path(__file__).resolve().parents[1]
ROOT = DESKTOP.parent
CACHE = DESKTOP / ".cache/signing"
PROFILE = "kikimimi-desktop-release"


def credentials():
    result = subprocess.run(["/usr/bin/security", "find-generic-password", "-s",
                             "dev.kikimimi.desktop.release", "-a", "signing-material-v1", "-w"],
                            capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("Desktop signing settings are not available in login Keychain")
    try:
        values = json.loads(result.stdout)
        required = ["APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD", "APPLE_ID", "APPLE_PASSWORD",
                    "TAURI_SIGNING_PRIVATE_KEY", "KIKIMIMI_UPDATER_PUBLIC_KEY"]
        if not all(isinstance(values.get(name), str) and values[name] for name in required):
            raise ValueError()
        return values
    except (ValueError, TypeError):
        raise RuntimeError("Saved signing settings are invalid; secret output suppressed") from None


def identity(values):
    env = os.environ.copy()
    env.update(values)
    openssl = Path("/opt/homebrew/opt/openssl@3/bin/openssl")
    if openssl.exists():
        env["OPENSSL_BIN"] = str(openssl)
    program = "import {deriveIdentity} from './desktop/scripts/apple-identity.mjs'; try {process.stdout.write(JSON.stringify(deriveIdentity()));} catch {process.exit(1);}"
    result = subprocess.run(["node", "--input-type=module", "-e", program],
                            cwd=ROOT, env=env, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("Cannot validate the saved Developer ID certificate; secret output suppressed")
    return json.loads(result.stdout)


def ensure_notary_profile(values, team_id, force=False):
    marker = CACHE / "notary-profile.json"
    if marker.exists() and not force:
        saved = json.loads(marker.read_text())
        if saved.get("team") == team_id:
            return PROFILE
    CACHE.mkdir(parents=True, exist_ok=True)
    # A controlling terminal lets notarytool read the password securely. The
    # password never appears in argv, the environment, or a transcript/log.
    pid, fd = pty.fork()
    if pid == 0:
        os.execv("/usr/bin/xcrun", ["xcrun", "notarytool", "store-credentials", PROFILE,
                                   "--apple-id", values["APPLE_ID"], "--team-id", team_id])
    sent = False
    prompt = bytearray()
    deadline = time.monotonic() + 90
    status = None
    try:
        while time.monotonic() < deadline:
            ready, _, _ = select.select([fd], [], [], 0.2)
            if ready:
                try:
                    data = os.read(fd, 4096)
                except OSError:
                    data = b""
                prompt.extend(data)
                if len(prompt) > 16384:
                    del prompt[:-8192]
            if not sent and b"password" in prompt.lower():
                if not termios.tcgetattr(fd)[3] & termios.ECHO:
                    os.write(fd, values["APPLE_PASSWORD"].encode() + b"\n")
                    sent = True
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                break
        else:
            os.kill(pid, signal.SIGTERM)
            os.waitpid(pid, 0)
            raise RuntimeError("Notarization-profile setup timed out; secret output suppressed")
        if status != 0 or not sent:
            raise RuntimeError("Notarization-profile setup failed; secret output suppressed")
    finally:
        prompt[:] = b"\0" * len(prompt)
        os.close(fd)
    marker.write_text(json.dumps({"profile": PROFILE, "team": team_id}) + "\n")
    print("Notarization credentials validated and saved in login Keychain", flush=True)
    return PROFILE
