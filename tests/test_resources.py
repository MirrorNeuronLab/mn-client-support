import json
from contextlib import contextmanager

from mn_sdk_shared.proto.runtime_control_pb2 import RuntimeReply
from mn_sdk_shared.runtime.control import CONTROL_VERSION
from mn_surface import RuntimeResources


def test_lists_jobs_through_saved_connection_across_pages():
    calls = []

    class Node:
        def call(self, method, action, arguments, *, request_id):
            calls.append((method, action, arguments["_context"]["query"].copy()))
            page = len(calls)
            result = {"items": [{"id": f"job-{page}"}]}
            if page == 1:
                result["next_page_token"] = "second"
            return RuntimeReply(
                version=CONTROL_VERSION, status=200, value_json=json.dumps(result)
            )

    @contextmanager
    def client_factory(connection, *, timeout):
        assert connection == "saved-profile"
        yield Node()

    resources = RuntimeResources("saved-profile", client_factory=client_factory)
    assert resources.items("/jobs") == [{"id": "job-1"}, {"id": "job-2"}]
    assert calls == [
        ("Read", "list_jobs", {}),
        ("Read", "list_jobs", {"page_token": "second"}),
    ]
