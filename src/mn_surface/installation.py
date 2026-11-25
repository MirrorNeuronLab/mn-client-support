"""Installation role and private connection settings for a runtime-free client."""

from __future__ import annotations

import json
import os
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import urlsplit, urlunsplit

from mn_sdk_shared.errors import AppError
from mn_sdk_shared.runtime.connection import installation_home


def _read_object(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(value, dict):
            raise ValueError
        return value
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as exc:
        raise AppError(
            "MN_CLIENT_CONFIG_INVALID",
            "Cannot read installation configuration.",
            hint="Repair the configuration in MN_HOME before continuing.",
            http_status=503,
        ) from exc


def is_client_install() -> bool:
    return (
        _read_object(installation_home() / "install_metadata.json").get("install_role")
        == "client"
    )


def normalize_api_url(value: str) -> str:
    try:
        value = value.strip()
        if "://" not in value:
            value = "http://" + value
            parsed = urlsplit(value)
            if parsed.port is None:
                value = urlunsplit(parsed._replace(netloc=parsed.netloc + ":54001"))
        parsed = urlsplit(value)
        valid = (
            parsed.scheme in {"http", "https"}
            and parsed.hostname
            and parsed.port != 0
            and not parsed.username
            and not parsed.password
            and not any(char.isspace() for char in value)
            and not parsed.query
            and not parsed.fragment
            and parsed.path.rstrip("/") in {"", "/api/v1"}
        )
    except (ValueError, AttributeError):
        valid = False
    if not valid:
        raise AppError(
            "MN_CLIENT_URL_INVALID",
            "Use a remote HTTP(S) API URL without credentials or query parameters.",
            exit_code=2,
            http_status=422,
        )
    return urlunsplit((parsed.scheme, parsed.netloc, "/api/v1", "", ""))


@dataclass(frozen=True)
class RemoteConnection:
    api_url: str
    token: str = field(repr=False)
    node_name: str = ""

    def public(self) -> dict:
        return {
            "mode": "client",
            "status": "connected",
            "api_url": self.api_url,
            "node_name": self.node_name,
        }


def load_connection() -> RemoteConnection:
    value = _read_object(installation_home() / "client.json")
    if not all(
        isinstance(value.get(key), str) and value[key]
        for key in ("api_url", "token", "node_name")
    ):
        raise AppError(
            "MN_CLIENT_NOT_CONNECTED",
            "No remote node is connected.",
            hint="Run mn node add <REMOTE_API_URL> --token <REMOTE_API_TOKEN>.",
            http_status=503,
        )
    return RemoteConnection(
        normalize_api_url(value["api_url"]), value["token"], value["node_name"]
    )


def save_connection(connection: RemoteConnection) -> None:
    home = installation_home()
    home.mkdir(parents=True, exist_ok=True, mode=0o700)
    # A single atomic private file prevents a reader observing a new URL with
    # the previous node's token. No process-global credential cache is used.
    fd, name = tempfile.mkstemp(prefix=".client-", dir=home)
    pending = Path(name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as output:
            json.dump(
                {
                    "api_url": connection.api_url,
                    "token": connection.token,
                    "node_name": connection.node_name,
                },
                output,
            )
            output.write("\n")
        pending.replace(home / "client.json")
    finally:
        pending.unlink(missing_ok=True)


def disconnect(node_name: str) -> dict:
    connection = load_connection()
    if node_name != connection.node_name:
        raise AppError(
            "MN_CLIENT_CONNECTION_MISMATCH",
            "Only the connected remote node can be disconnected.",
            exit_code=2,
            http_status=409,
        )
    (installation_home() / "client.json").unlink(missing_ok=True)
    return {"mode": "client", "node_name": node_name, "status": "disconnected"}


def local_api_token() -> str:
    token = os.environ.get("MN_API_TOKEN", "").strip()
    if not token:
        try:
            token = (
                (installation_home() / "api.token").read_text(encoding="utf-8").strip()
            )
        except OSError:
            pass
    if not token:
        raise AppError(
            "MN_CLIENT_AUTH_REQUIRED",
            "The client API requires a local bearer token.",
            hint="Set MN_API_TOKEN or restore MN_HOME/api.token.",
            http_status=503,
        )
    return token
