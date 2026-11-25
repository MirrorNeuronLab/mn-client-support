"""REST transport shared by lightweight clients. Never prepares local jobs."""

from __future__ import annotations

from contextlib import contextmanager
from pathlib import Path
from urllib.parse import quote

import httpx

from mn_sdk_shared.errors import AppError, sanitize_context

from .installation import (
    RemoteConnection,
    load_connection,
    normalize_api_url,
    save_connection,
)

API_CONTRACT = "mirrorneuron.rest.v1"


def resource_id(value: str) -> str:
    return quote(str(value), safe="")


def transport_error() -> AppError:
    return AppError(
        "MN_REMOTE_API_UNAVAILABLE",
        "The remote API could not complete the request.",
        hint="Check the remote connection. Inspect existing jobs/runs before retrying a submission.",
        http_status=503,
    )


class RemoteApiClient:
    def __init__(self, connection: RemoteConnection | None = None, *, transport=None):
        self._connection = connection
        self.transport = transport

    @property
    def connection(self) -> RemoteConnection:
        return self._connection or load_connection()

    def _client(self) -> httpx.Client:
        # Read URL and credential together once for this request/transaction.
        # A later request observes a successful reconnect, including pollers.
        connection = self.connection
        return httpx.Client(
            base_url=connection.api_url + "/",
            headers={"Authorization": f"Bearer {connection.token}"},
            timeout=httpx.Timeout(60, connect=10),
            follow_redirects=False,
            transport=self.transport,
        )

    @staticmethod
    def _path(path: str) -> str:
        if not path.startswith("/") or path.startswith("//") or ".." in path.split("/"):
            raise ValueError("Expected an API-relative resource path")
        return path.lstrip("/")

    def _check(self, response: httpx.Response) -> None:
        if response.headers.get("x-mn-install-role") == "client":
            raise AppError(
                "MN_REMOTE_API_PROTOCOL",
                "Connect directly to a runtime node, not another client relay.",
                http_status=412,
            )
        if 200 <= response.status_code < 300:
            return
        status = response.status_code
        message = {
            401: "The remote API rejected its stored API token.",
            403: "The remote API denied this operation.",
            404: "The requested remote resource is unavailable.",
        }.get(status, "The remote API rejected the request.")
        try:
            problem = response.json()
        except ValueError:
            problem = {}
        # Retain structured field errors but never raw upstream bodies/headers.
        details = (
            sanitize_context({"errors": problem.get("errors", [])})
            if isinstance(problem, dict)
            else {}
        )
        raise AppError(
            "MN_REMOTE_API_ERROR",
            message,
            details=details,
            http_status=status,
            hint="Check the remote API and the supplied configuration.",
            exit_code=2 if status == 422 else 1,
        )

    def request(self, method: str, path: str, **kwargs):
        try:
            with self._client() as client:
                response = client.request(method, self._path(path), **kwargs)
                self._check(response)
                if not response.content:
                    return None
                return response.json()
        except httpx.RequestError:
            raise transport_error() from None
        except ValueError:
            raise AppError(
                "MN_REMOTE_API_PROTOCOL",
                "The remote API returned an invalid response.",
                http_status=502,
            ) from None

    def items(self, path: str, **params) -> list:
        result, seen = [], set()
        while True:
            response = self.request(
                "GET", path, params={k: v for k, v in params.items() if v is not None}
            )
            if not isinstance(response, dict) or not isinstance(
                response.get("items"), list
            ):
                raise AppError(
                    "MN_REMOTE_API_PROTOCOL",
                    "The remote API returned an invalid collection.",
                    http_status=502,
                )
            result.extend(response.get("items", []))
            token = response.get("next_page_token")
            if not token:
                return result
            if not isinstance(token, str) or token in seen:
                raise AppError(
                    "MN_REMOTE_API_PROTOCOL",
                    "The remote API repeated a page token.",
                    http_status=502,
                )
            seen.add(token)
            params["page_token"] = token

    def conditional(self, method: str, path: str, **kwargs):
        """Use the server's ETag without recomputing or bypassing its fence."""
        try:
            with self._client() as client:
                current = client.get(self._path(path), params=kwargs.get("params"))
                self._check(current)
                etag = current.headers.get("etag")
                if not etag:
                    raise AppError(
                        "MN_REMOTE_API_PROTOCOL",
                        "The remote resource is missing its revision fence.",
                        http_status=502,
                    )
                headers = {**kwargs.pop("headers", {}), "If-Match": etag}
                response = client.request(
                    method, self._path(path), headers=headers, **kwargs
                )
                self._check(response)
                return response.json() if response.content else None
        except httpx.RequestError:
            raise transport_error() from None

    @contextmanager
    def stream(self, path: str, *, method: str = "GET", **kwargs):
        try:
            with self._client() as client:
                with client.stream(
                    method,
                    self._path(path),
                    timeout=httpx.Timeout(None, connect=10),
                    **kwargs,
                ) as response:
                    if response.status_code >= 300:
                        response.read()
                    self._check(response)
                    yield response
        except httpx.RequestError:
            raise transport_error() from None

    def download(self, path: str, destination: str | Path, *, method="GET") -> dict:
        target = Path(destination).expanduser()
        # An incomplete transfer must not replace an existing user file.
        import os
        import tempfile

        fd, name = tempfile.mkstemp(prefix=".mn-download-", dir=target.parent)
        pending = Path(name)
        try:
            with (
                os.fdopen(fd, "wb") as output,
                self.stream(path, method=method) as response,
            ):
                for chunk in response.iter_bytes():
                    output.write(chunk)
            pending.replace(target)
        finally:
            pending.unlink(missing_ok=True)
        return {"path": str(target)}


def connect(api_url: str, token: str, *, transport=None) -> dict:
    if not token.strip() or any(char.isspace() for char in token):
        raise AppError(
            "MN_CLIENT_TOKEN_INVALID",
            "Supply the remote API bearer token.",
            exit_code=2,
            http_status=422,
        )
    candidate = RemoteConnection(normalize_api_url(api_url), token)
    client = RemoteApiClient(candidate, transport=transport)
    health = client.request("GET", "/health")
    if (
        not isinstance(health, dict)
        or health.get("api_contract") != API_CONTRACT
        or health.get("mode") == "client"
    ):
        raise AppError(
            "MN_REMOTE_API_PROTOCOL",
            "Connect directly to a runtime node exposing mirrorneuron.rest.v1.",
            http_status=412,
        )
    summary = client.request("GET", "/system/summary")
    identity = summary.get("identity", {}) if isinstance(summary, dict) else {}
    node = identity.get("actual") if isinstance(identity, dict) else None
    if not isinstance(node, str) or not node or identity.get("valid") is not True:
        raise AppError(
            "MN_REMOTE_NODE_INVALID",
            "The remote API did not report a valid runtime node identity.",
            http_status=412,
        )
    connection = RemoteConnection(candidate.api_url, token, node)
    save_connection(connection)
    return connection.public()
