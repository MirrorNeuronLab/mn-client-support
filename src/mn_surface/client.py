"""Runtime-backed Core read contract for surface integrations such as MCP."""

from mn_sdk_shared.runtime.methods import CORE_METHODS
from mn_sdk_shared.runtime.control import RuntimeControlClient


class SurfaceRuntimeClient:
    @property
    def channel(self):
        return self

    def close(self):
        pass  # Each operation owns and closes its channel.

    def __getattr__(self, method):
        if method not in CORE_METHODS:
            raise AttributeError(method)

        def call(*args, **kwargs):
            with RuntimeControlClient() as client:
                return client.read("core." + method, {"args": args, "kwargs": kwargs})

        return call
