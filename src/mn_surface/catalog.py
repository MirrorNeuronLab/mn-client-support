"""Read the application surface's catalog without a runtime connection."""

from pathlib import Path
from subprocess import TimeoutExpired

from mn_sdk_shared.errors import AppError
from mn_sdk_shared.runtime_config import resolve_mn_home
from mn_sdk_shared.runtime_contract.public import public_value
from mn_sdk_shared.storage.transactions import file_transaction

from mn_sdk_shared.blueprints.sources import (
    BlueprintCatalogError,
    filter_blueprints_by_category,
    load_active_blueprint_catalog,
)


def list_catalog_blueprints(
    category: str | None = None, *, repository: str | None = None
) -> list[dict]:
    """Return display metadata from the configured local or Git catalog."""
    try:
        config = None
        if repository:
            local = Path(repository).expanduser()
            config = (
                {"source": "local", "local": str(local)}
                if local.is_dir()
                else {"source": "github", "repo": repository}
            )
        home = resolve_mn_home()
        # CLI and API can read the same catalog during a first Git checkout.
        with file_transaction(home / ".surface-catalog"):
            catalog = load_active_blueprint_catalog(config, mn_home=home)
    except (BlueprintCatalogError, OSError, ValueError, TimeoutExpired):
        raise AppError(
            "MN_BLUEPRINT_CATALOG_UNAVAILABLE",
            "Couldn't read the blueprint catalog. Check the configured source and try again.",
            hint="Git catalogs require Git and access to the configured repository.",
            http_status=503,
        ) from None

    blueprints = filter_blueprints_by_category(catalog.blueprints, category)
    return sorted(
        (public_value(item) for item in blueprints), key=lambda item: item["id"]
    )
