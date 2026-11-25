import json
import stat

import httpx
import pytest

from mn_sdk_shared.errors import AppError
from mn_surface.installation import (
    RemoteConnection,
    disconnect,
    is_client_install,
    load_connection,
    normalize_api_url,
    save_connection,
)
from mn_surface.remote_api import API_CONTRACT, RemoteApiClient, connect


@pytest.fixture
def client_home(tmp_path, monkeypatch):
    monkeypatch.setenv("MN_HOME", str(tmp_path))
    (tmp_path / "install_metadata.json").write_text('{"install_role":"client"}')
    return tmp_path


def handshake(request):
    assert request.headers["authorization"] == "Bearer secret"
    if request.url.path.endswith("/health"):
        return httpx.Response(200, json={"api_contract": API_CONTRACT})
    assert request.url.path == "/api/v1/system/summary"
    return httpx.Response(
        200, json={"identity": {"actual": "remote@host", "valid": True}}
    )


def test_connection_is_atomic_private_and_failed_reconnect_preserves_it(client_home):
    assert is_client_install()
    public = connect(
        "https://remote", "secret", transport=httpx.MockTransport(handshake)
    )
    assert public["node_name"] == "remote@host"
    assert "secret" not in json.dumps(public)
    path = client_home / "client.json"
    before = path.read_bytes()
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    with pytest.raises(AppError) as error:
        connect(
            "https://other",
            "invalid",
            transport=httpx.MockTransport(lambda _: httpx.Response(401)),
        )
    assert error.value.http_status == 401
    assert path.read_bytes() == before
    assert "secret" not in repr(load_connection())
    with pytest.raises(AppError):
        disconnect("another-node")
    assert path.exists()
    disconnect("remote@host")
    with pytest.raises(AppError, match="No remote node"):
        load_connection()


@pytest.mark.parametrize(
    "url",
    [
        "ftp://host",
        "host:bad",
        "http://host:0",
        "https://u:p@host",
        "https://host/?token=x",
        "https://host/other",
        "http://bad host",
    ],
)
def test_invalid_urls_are_actionable(url):
    with pytest.raises(AppError) as error:
        normalize_api_url(url)
    assert error.value.code == "MN_CLIENT_URL_INVALID"


@pytest.mark.parametrize(
    "payload", [None, [], {}, {"identity": {"actual": "unnamed", "valid": False}}]
)
def test_invalid_remote_identity_never_saves(client_home, payload):
    def remote(request):
        return (
            handshake(request)
            if request.url.path.endswith("/health")
            else httpx.Response(200, json=payload)
        )

    with pytest.raises(AppError):
        connect("https://remote", "secret", transport=httpx.MockTransport(remote))
    assert not (client_home / "client.json").exists()


def test_client_relay_cannot_be_used_as_runtime(client_home):
    with pytest.raises(AppError, match="runtime node"):
        connect(
            "http://localhost",
            "secret",
            transport=httpx.MockTransport(
                lambda _: httpx.Response(
                    200,
                    headers={"X-MN-Install-Role": "client"},
                    json={"api_contract": API_CONTRACT},
                )
            ),
        )
    assert not (client_home / "client.json").exists()


def test_pagination_conditional_requests_and_download(client_home):
    save_connection(RemoteConnection("https://remote/api/v1", "secret", "node"))
    calls = []

    def remote(request):
        calls.append(request)
        if request.url.path.endswith("/jobs"):
            return httpx.Response(
                200,
                json={
                    "items": [{"id": len(calls)}],
                    "next_page_token": "second" if len(calls) == 1 else None,
                },
            )
        if request.url.path.endswith("/output"):
            return httpx.Response(200, content=b"result bytes")
        if request.method == "GET":
            return httpx.Response(
                200, headers={"ETag": '"revision-3"'}, json={"id": "job"}
            )
        assert request.headers["if-match"] == '"revision-3"'
        return httpx.Response(204)

    api = RemoteApiClient(transport=httpx.MockTransport(remote))
    assert len(api.items("/jobs")) == 2
    assert calls[1].url.params["page_token"] == "second"
    assert api.conditional("DELETE", "/jobs/job") is None
    api.download("/output", client_home / "output")
    assert (client_home / "output").read_bytes() == b"result bytes"
    assert all(r.headers["authorization"] == "Bearer secret" for r in calls)


def test_transport_failure_does_not_resubmit_or_overwrite_download(client_home):
    calls = []

    def remote(request):
        calls.append(request)
        raise httpx.ReadError("remote disconnected", request=request)

    api = RemoteApiClient(
        RemoteConnection("https://remote/api/v1", "secret", "node"),
        transport=httpx.MockTransport(remote),
    )
    with pytest.raises(AppError) as error:
        api.request("POST", "/blueprints/bp/runs", json={})
    assert "before retrying" in error.value.hint
    assert len(calls) == 1
    target = client_home / "result"
    target.write_text("previous result")
    with pytest.raises(AppError):
        api.download("/result", target)
    assert target.read_text() == "previous result"
    assert not list(client_home.glob(".mn-download-*"))


def test_existing_transport_uses_reconnected_url_and_token_together(client_home):
    save_connection(
        RemoteConnection("https://first/api/v1", "first-token", "first-node")
    )
    observed = []

    def remote(request):
        observed.append((request.url.host, request.headers["authorization"]))
        return httpx.Response(200, json={"items": []})

    api = RemoteApiClient(transport=httpx.MockTransport(remote))
    api.request("GET", "/nodes")
    save_connection(
        RemoteConnection("https://second/api/v1", "second-token", "second-node")
    )
    api.request("GET", "/nodes")
    assert observed == [
        ("first", "Bearer first-token"),
        ("second", "Bearer second-token"),
    ]
    disconnect("second-node")
    with pytest.raises(AppError, match="No remote node"):
        api.request("GET", "/nodes")
    assert len(observed) == 2
