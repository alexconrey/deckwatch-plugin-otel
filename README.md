# deckwatch-plugin-otel

A [deckwatch](https://github.com/alexconrey/deckwatch) plugin that provisions an OpenTelemetry
Collector sidecar for an application via the **Infrastructure tab** — no annotations required.

## Usage

1. Open the Application detail page in deckwatch.
2. Click the **Infrastructure** tab.
3. Click **+ Add OpenTelemetry Collector**.
4. Optionally set a service name, then click **Provision**.

Once provisioned, deckwatch injects on the next reconcile:

- **Sidecar**: `otel/opentelemetry-collector:latest` on port 4317 (100m CPU / 128Mi memory),
  named `otel-collector`
- **Env vars** into every deployment in the application:
  - `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317`
  - `OTEL_SERVICE_NAME=<value>` — only when the field is non-empty
- **Annotation**: `deckwatch.io/otel: "provisioned"` on all deployments

To remove the sidecar, click **Deprovision** on the Infrastructure tab. The sidecar is removed
from all deployments on the next reconcile via server-side apply.

## Configuration

### Resource fields (per-provisioned instance)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `OTEL_IMAGE` | String | `otel/opentelemetry-collector:latest` | OTel Collector container image |
| `OTEL_SERVICE_NAME` | String | _(empty)_ | Injected as `OTEL_SERVICE_NAME` into all deployments when set. Leave empty for per-service control. |

### Plugin config (global defaults)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `OTEL_IMAGE` | String | `otel/opentelemetry-collector:latest` | Default OTel Collector image (overridable per-resource) |

Set global defaults in **Settings → Plugins → OpenTelemetry → Configure**.

## Building

```bash
# Install the wasm32 target (once)
rustup target add wasm32-unknown-unknown

# Build the WASM artifact
make build          # → dist/plugin.wasm

# Run host-target unit tests
make test
```

## Marketplace

This plugin is available on the [deckwatch marketplace](http://market.deckwatch.io).

To install via the deckwatch UI: **Settings → Plugins → Browse Marketplace → OpenTelemetry**.

### Submitting to the marketplace

To add or update this plugin in the marketplace catalog, open a PR against
[alexconrey/deckwatch-marketplace](https://github.com/alexconrey/deckwatch-marketplace)
adding the following entry to `catalog.json`:

```json
{
  "name": "OpenTelemetry Collector",
  "slug": "deckwatch-plugin-otel",
  "description": "Provisions an OpenTelemetry Collector sidecar via the Application Infrastructure tab.",
  "author": "alexconrey",
  "homepage": "https://github.com/alexconrey/deckwatch-plugin-otel",
  "trust_level": "community",
  "tags": ["otel", "opentelemetry", "tracing", "observability", "sidecar"],
  "latest_version": "0.1.0",
  "source": {
    "type": "github",
    "repo": "alexconrey/deckwatch-plugin-otel",
    "ref": "v0.1.0",
    "path": "plugin.wasm",
    "use_release": true
  },
  "allowed_hosts_hint": []
}
```

Also add `plugins/deckwatch-plugin-otel.md` following the template in the marketplace repo.

## License

Apache-2.0
