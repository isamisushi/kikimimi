"""Exercise public app initialization, including updater plugin registration."""
import subprocess
import tempfile


def check_startup(app, env, local):
    was_open = local.close_installed_app(env)
    try:
        processes = subprocess.check_output(["ps", "-axo", "comm="], env=env, text=True)
        if any(line.strip().endswith("/kikimimi-desktop") for line in processes.splitlines()):
            raise RuntimeError("Quit other kikimimi app copies before checking startup")
        with tempfile.TemporaryFile(mode="w+") as output:
            process = subprocess.Popen([str(app / "Contents/MacOS/kikimimi-desktop")],
                                       env=env, stdout=output, stderr=output)
            try:
                try:
                    code = process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    print("PASS: packaged public app remained running through startup", flush=True)
                else:
                    output.seek(0)
                    raise RuntimeError(f"Packaged app exited during startup ({code}): {output.read()[-4000:]}")
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
    finally:
        if was_open:
            subprocess.run(["open", "-a", str(local.INSTALLED)], env=env, check=True)


if __name__ == "__main__":
    import argparse
    import importlib.util
    import os
    from pathlib import Path

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    args = parser.parse_args()
    spec = importlib.util.spec_from_file_location("local", Path(__file__).with_name("install-local.py"))
    local = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(local)
    env = {k: v for k, v in os.environ.items() if not k.startswith(("APPLE_", "TAURI_SIGNING_"))}
    check_startup(args.app.resolve(), env, local)
