"""Application-side access to MirrorNeuron runtime nodes."""

from mn_sdk_shared.runtime.connection import connect
from mn_sdk_shared.runtime.control import RuntimeControlClient

from .resources import RuntimeResources

__all__ = ["RuntimeControlClient", "RuntimeResources", "connect"]
