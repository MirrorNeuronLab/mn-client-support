"""Invoke an installed lifecycle executable without importing node packages."""

import json
import os
import subprocess

from mn_sdk_shared.errors import AppError

from mn_sdk_shared.runtime.connection import installation_home, installation_role


def run(action, options=None, *, command_runner=subprocess.run):
    if installation_role() == "client":
        raise AppError(
            "MN_RUNTIME_NOT_INSTALLED",
            "No runtime is installed in client mode.",
            hint="Connect to a runtime node with mn node add. Start the optional API with mn-api.",
            exit_code=2,
        )
    home = installation_home()
    python = home / "venv/bin/python"
    if not python.is_file():
        raise AppError(
            "MN_RUNTIME_NOT_INSTALLED",
            "The installed runtime lifecycle executable is missing.",
            hint="Reinstall this installation in the same role.",
        )
    command = [str(python), "-m", "mn_runtime.lifecycle", action, "--json"]
    for key, value in (options or {}).items():
        if key in {"host", "grpc_port", "timeout"} and value is not None:
            command.extend(["--" + key.replace("_", "-"), str(value)])
        elif (
            key
            in {
                "api",
                "web_ui",
                "dry_run",
                "include_cache",
                "force",
                "yes",
                "check_only",
                "repair",
            }
            and value is True
        ):
            command.append("--" + key.replace("_", "-"))
    result = command_runner(
        command,
        env={**os.environ, "MN_HOME": str(home)},
        capture_output=True,
        text=True,
    )
    try:
        value = json.loads(result.stdout.strip().splitlines()[-1])
    except (IndexError, ValueError) as exc:
        raise AppError(
            "MN_RUNTIME_LIFECYCLE_FAILED",
            "The installed runtime command did not return a status.",
            hint="Inspect the runtime logs in MN_HOME/logs.",
        ) from exc
    if result.returncode:
        raise AppError(
            value.get("code", "MN_RUNTIME_LIFECYCLE_FAILED"),
            value.get("message", "The runtime command failed."),
            details=value.get("details"),
        )
    return value
