# MirrorNeuron Surface SDK

The `mirrorneuron-surface` Python distribution is the application client used
by the MirrorNeuron CLI and REST API. It can also be used directly from Python.
It uses `mirrorneuron-sdk-contracts` for the wire protocol, resource schemas,
and catalog format. A remote runtime handles execution.

## Install from source

Use Python 3.12 or later. In a workspace containing this repository and a
matching `mn-python-sdk` checkout with `packages/contracts`, run the following
from `mn-surface` in an activated virtual environment. The local paths supply
the shared dependencies while their wheels await publication.

```sh
python -m pip install -e ../mn-python-sdk/packages/common \
  -e ../mn-python-sdk/packages/contracts -e .
```

## Use the client

After connecting to a node with the CLI, use its saved profile from Python.

```python
from mn_surface import RuntimeResources

resources = RuntimeResources()
jobs = resources.items("/jobs")
```

`mn_surface.connect` attaches to a node programmatically.
`mn_surface.catalog.list_catalog_blueprints` can read a local blueprint catalog
before attaching to a runtime. For the older HTTP client in `mn_surface.remote_api`,
install the `http` extra.

## Run the tests

```sh
python -m pip install -e '.[http]' pytest
python -m pytest
```
