"""Resource adapter over the native runtime protocol; no HTTP connection."""

import json as jsonlib
from contextlib import contextmanager
from pathlib import Path
from urllib.parse import quote

from mn_sdk_shared.errors import AppError
from mn_sdk_shared.runtime_contract.endpoints import (
    arguments,
    endpoint,
    rpc_method,
    rpc_timeout,
)

from mn_sdk_shared.runtime.stream import ResourceStream
from mn_sdk_shared.runtime.control import RuntimeControlClient, decode_reply


def resource_id(value):
    return quote(str(value), safe="")


class RuntimeResources:
    def __init__(self, connection=None, *, client_factory=RuntimeControlClient):
        self.connection = connection
        self.client_factory = client_factory

    @staticmethod
    def _arguments(method, path, params, body, headers):
        item, variables, query = endpoint(method, path)
        query.update(
            {key: value for key, value in (params or {}).items() if value is not None}
        )
        return item, arguments(
            item, path=variables, query=query, body=body, headers=headers
        )

    def raw_request(self, method, path, *, params=None, json=None, headers=None):
        item, values = self._arguments(method, path, params, json, headers)
        values["_context"] = {
            "method": method,
            "path": path,
            "headers": headers or {},
            "query": params or {},
        }
        with self.client_factory(
            self.connection, timeout=rpc_timeout(item, values)
        ) as client:
            return client.call(
                rpc_method(item),
                item["action"],
                values,
                request_id=(headers or {}).get("Idempotency-Key", ""),
            )

    @staticmethod
    def check(reply):
        value = decode_reply(reply)
        if reply.status >= 400:
            problem = value if isinstance(value, dict) else {}
            raise AppError(
                problem.get("code", "MN_RUNTIME_OPERATION_FAILED"),
                problem.get("detail", "The runtime rejected the operation."),
                details={"errors": problem.get("errors", [])},
                http_status=reply.status,
            )
        return value

    def request(self, method, path, **kwargs):
        return self.check(self.raw_request(method, path, **kwargs))

    def items(self, path, **params):
        items, seen = [], set()
        while True:
            result = self.request("GET", path, params=params)
            if not isinstance(result, dict) or not isinstance(
                result.get("items"), list
            ):
                raise AppError(
                    "MN_RUNTIME_PROTOCOL_ERROR",
                    "Invalid resource collection.",
                    http_status=502,
                )
            items.extend(result["items"])
            token = result.get("next_page_token")
            if not token:
                return items
            if not isinstance(token, str) or token in seen:
                raise AppError(
                    "MN_RUNTIME_PROTOCOL_ERROR",
                    "Invalid collection continuation.",
                    http_status=502,
                )
            seen.add(token)
            params["page_token"] = token

    def conditional(self, method, path, **kwargs):
        # Pin both steps to one profile so reconnect cannot change the node
        # between reading a revision fence and applying the mutation.
        from mn_sdk_shared.runtime.connection import load_connection

        pinned = RuntimeResources(
            self.connection or load_connection(), client_factory=self.client_factory
        )
        current = pinned.raw_request("GET", path, params=kwargs.get("params"))
        self.check(current)
        etag = {key.lower(): value for key, value in current.metadata.items()}.get(
            "etag"
        )
        if not etag:
            raise AppError(
                "MN_RUNTIME_PROTOCOL_ERROR",
                "Missing resource revision fence.",
                http_status=502,
            )
        return pinned.request(
            method,
            path,
            headers={**kwargs.pop("headers", {}), "If-Match": etag},
            **kwargs,
        )

    @contextmanager
    def stream(self, path, *, method="GET", params=None, headers=None, json=None):
        item, values = self._arguments(method, path, params, json, headers)
        values["_stream"] = {
            "method": method,
            "path": path,
            "headers": headers or {},
            "query": params or {},
        }
        with self.client_factory(self.connection) as client:
            with client.stream(
                rpc_method(item, streaming=True), item["action"], values
            ) as frames:
                response = ResourceStream(frames)
                if response.status_code >= 400:
                    problem = jsonlib.loads(response.read())
                    raise AppError(
                        problem.get("code", "MN_RUNTIME_OPERATION_FAILED"),
                        problem.get("detail", "The runtime rejected the request."),
                        http_status=response.status_code,
                    )
                yield response

    def download(self, path, destination, *, method="GET"):
        import os
        import tempfile

        target = Path(destination).expanduser()
        fd, name = tempfile.mkstemp(prefix=".mn-download-", dir=target.parent)
        try:
            with (
                os.fdopen(fd, "wb") as output,
                self.stream(path, method=method) as response,
            ):
                if response.status_code >= 400:
                    problem = jsonlib.loads(response.read())
                    raise AppError(
                        problem.get("code", "MN_RUNTIME_DOWNLOAD_FAILED"),
                        problem.get("detail", "Download failed."),
                        http_status=response.status_code,
                    )
                for chunk in response.iter_bytes():
                    output.write(chunk)
            Path(name).replace(target)
        finally:
            Path(name).unlink(missing_ok=True)
        return {"path": str(target)}
