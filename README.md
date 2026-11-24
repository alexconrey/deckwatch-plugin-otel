# deckwatch-plugin-otel

A [deckwatch](https://github.com/alexconrey/deckwatch) plugin that adds an OpenTelemetry
Collector sidecar to a deployment, or to every workload in an application, and points the
application's OpenTelemetry SDK at it. It replaces the `otel-collector` addon that deckwatch
removed in #171.

When enabled, the plugin adds:

- **An `otel-collector` sidecar.** It runs a pinned `otel/opentelemetry-collector-contrib`
  image whose OTLP gRPC (4317) and HTTP (4318) receivers listen on **127.0.0.1 only**, so the
  application container can send to it but other pods cannot. Data flows through
  `memory_limiter` and then `batch` before the `otlp_grpc` or `otlp_http` exporter sends it to
  your backend.
- **OTEL environment variables on the application container**:
  `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_PROTOCOL`, `OTEL_SERVICE_NAME` (the
  deployment name), `OTEL_RESOURCE_ATTRIBUTES` (`k8s.namespace.name`, `k8s.deployment.name`,
  and `deckwatch.application.name` when it is known) and `OTEL_<SIGNAL>_EXPORTER`. If you
  already set one of these variables yourself, your value is kept.
- **A ConfigMap** (`<name>-otelcol`) with the collector configuration. If exporter headers are
  configured, it also adds **a Secret** (`<name>-otelcol-headers`) with their values. Both are
  created in the workload's namespace.

## Requirements

| | Version |
|---|---|
| deckwatch | **0.6.4 or later.** The plugin declares `min_deckwatch_version: 0.6.4`. Releases before 0.6.4 do not check that field and drop sidecar volumes, so the collector would start with the image's default configuration, which listens on 0.0.0.0 and exports nowhere. Do not install this plugin on them. |
| deckwatch-plugin-sdk | `main` at `d4c0280`: v0.9.0 plus `McpTuningField`. deckwatch 0.6.4+ also accepts sidecar volumes, separate requests and limits, and `min_deckwatch_version`, which are not in the SDK yet (SDK PRs #13 and #11). [`src/host.rs`](src/host.rs) emits them until the SDK ships them. |
| Collector image | The default is `otel/opentelemetry-collector-contrib:0.165.0` pinned by digest. Custom images must be collector **0.144.0 or later**, because that release introduced the `otlp_grpc` and `otlp_http` exporter names. |

## Install

1. In deckwatch, open **Settings → Plugins** and add the plugin. Name it `otel`, so its
   annotations read `deckwatch.plugin-env/otel`, and pin the release checksum:

   ```json
   {
     "name": "otel",
     "enabled": true,
     "source": {
       "type": "github",
       "repo": "alexconrey/deckwatch-plugin-otel",
       "ref": "v0.2.0",
       "path": "plugin.wasm",
       "use_release": true
     },
     "sha256": "<plugin.wasm.sha256 from the release>",
     "allowed_hosts": []
   }
   ```

   Leave **`allowed_hosts` empty**. The plugin never makes outbound requests, and the module
   does not import extism's HTTP host function at all. CI checks this.

2. Open **Settings → Plugins → otel → Configure** and set at least **Exporter endpoint**
   (`OTLP_ENDPOINT`). Everything else has a default.

## Configuration

These settings apply to the whole plugin. deckwatch passes only saved values to the plugin, so
the plugin applies the defaults itself.

| Key | Type | Default | Description |
|---|---|---|---|
| `OTLP_ENDPOINT` | String, **required** | — | Where every sidecar sends telemetry. This is usually the same value as deckwatch's `tracing.otlp_endpoint`, which plugins cannot read. For gRPC use `host:port` (`tempo.observability.svc:4317`); for HTTP use a base URL (`https://otlp.example.com`). |
| `OTLP_PROTOCOL` | Select | `grpc` | Exporter protocol: `grpc` (the `otlp_grpc` exporter) or `http/protobuf` (`otlp_http`). |
| `OTLP_INSECURE` | Bool | `false` | Export without TLS, like deckwatch's `tracing.otlp_insecure`. This is typical for an in-cluster Tempo or Jaeger. An `http://` endpoint requires `true` and an `https://` endpoint requires `false`, so plaintext is never enabled by accident. |
| `OTLP_HEADERS` | **Secret** | — | Exporter headers, usually credentials, as comma-separated `name=value` pairs in the `OTEL_EXPORTER_OTLP_HEADERS` format. Values may be %-encoded. See [Exporter headers](#exporter-headers). |
| `SIGNALS` | String | `traces` | The signals the collector forwards: any of `traces`, `metrics`, `logs`. |
| `APP_OTLP_PROTOCOL` | Select | `grpc` | How the application reaches the sidecar: `grpc` (`http://localhost:4317`) or `http/protobuf` (`http://localhost:4318`). The plugin sets `OTEL_EXPORTER_OTLP_PROTOCOL` to match, so SDKs whose default is the other protocol still connect. |
| `COLLECTOR_IMAGE` | String | `otel/opentelemetry-collector-contrib:0.165.0@sha256:e40d61f1…` | Must be pinned. `:latest` and untagged images are rejected, and a tag without a digest produces a warning. |
| `COLLECTOR_CONFIG_DIR` | String | derived | The directory the image reads `config.yaml` from. By default it is derived from the image name: `/etc/otelcol-contrib`, or `/etc/otelcol` for the core image. You only need to set it for custom images. |
| `COLLECTOR_CPU_REQUEST` / `COLLECTOR_CPU_LIMIT` | String | `100m` / `500m` | Sidecar CPU. Set `none` to leave a value unset. A request may not exceed its limit. |
| `COLLECTOR_MEMORY_REQUEST` / `COLLECTOR_MEMORY_LIMIT` | String | `128Mi` / `256Mi` | Sidecar memory. The `memory_limiter` percentages are relative to the limit. |
| `BATCH_TIMEOUT` | String | `200ms` | `batch`: the longest a batch waits before it is sent. |
| `BATCH_SEND_SIZE` | String | `8192` | `batch`: the number of items that triggers a send. |
| `BATCH_MAX_SIZE` | String | — | `batch`: the upper bound on a batch; larger batches are split. Empty or `0` means no limit. |
| `MEMORY_CHECK_INTERVAL` | String | `1s` | `memory_limiter`: how often memory use is measured. |
| `MEMORY_LIMIT_PERCENTAGE` | String | `80` | `memory_limiter`: the hard limit, as a percentage of the sidecar memory limit. |
| `MEMORY_SPIKE_PERCENTAGE` | String | `25` | `memory_limiter`: headroom below the hard limit. Must be lower than the limit percentage. |

When the configuration is invalid, the plugin injects nothing. For a deployment, deckwatch
logs the errors. For an application, provisioning is rejected and the errors are shown.
Error messages never repeat the header values.

### Exporter headers

`OTLP_HEADERS` is a `Secret` field. The plugin writes the header values into the Secret
`<name>-otelcol-headers` in the workload's namespace (base64 `data`), mounts it read-only at
`/etc/otelcol-secrets`, and the collector configuration refers to each header as
`${file:/etc/otelcol-secrets/<header>}`. The values never appear in the pod spec, in an
environment variable, in the ConfigMap, or in the resource rows deckwatch stores for an
application.

> **Known deckwatch issue:** if you save `OTLP_HEADERS` from the plugin's settings form,
> deckwatch stores it encrypted and then passes the **ciphertext** to the plugin without
> decrypting it. The plugin detects this, refuses the value and reports an error. Until that
> is fixed in deckwatch, set the value in one of these ways:
>
> - **Recommended:** add an `OTLP_HEADERS` environment variable to the deckwatch Deployment
>   from a Kubernetes Secret, then set `"inherit_env_keys": ["OTLP_HEADERS"]` on the plugin.
>   With this approach the value is never stored in deckwatch's database.
> - Use the `update_plugin_config` MCP tool or `PUT /api/settings`. deckwatch then stores the
>   value unencrypted but masks it when settings are read.

The Secret is mounted into the sidecar only, never into the application container. After you
rotate a header value, restart the workloads (for example with `restart_application`): the
collector reads its configuration only at startup, and the value is deliberately left out of
the hash that triggers a rollout.

## Enable for a deployment

1. Opt the deployment in with the annotation (or a pod template label) `deckwatch.io/otel:
   enabled`. A label is the only way to enable it when you create the deployment, because
   deckwatch's create API takes labels but not annotations.

   ```yaml
   metadata:
     annotations:
       deckwatch.io/otel: enabled
   ```

2. Save the deployment through deckwatch's deployment API: the deployment edit form in the
   UI, or `POST`/`PUT /api/namespaces/{ns}/deployments[/{name}]`. These are the only paths that
   run plugins. The YAML editor, the MCP `create_deployment`/`update_deployment` tools (which
   go through mcp-k8s), `kubectl` and GitOps do not.

Each time deckwatch saves the deployment, the plugin returns the same output, and deckwatch
skips containers and env vars that already exist. Repeated saves therefore change nothing
and cause no restarts.

**Disable:** set `deckwatch.io/otel: disabled` (or remove the opt-in) and save. The plugin
then returns nothing. **However, deckwatch 0.6.x does not remove what a plugin added
earlier**, even though it records it in the `deckwatch.plugin-env/otel` and
`deckwatch.plugin-sidecar/otel` annotations. Until that is fixed, remove the following by
hand in the deployment YAML:

- the `otel-collector` container;
- the `otelcol-config` and `otelcol-headers` volumes;
- the env vars listed in `deckwatch.plugin-env/otel`, and that annotation itself.

Then delete the ConfigMap and Secret:

```sh
kubectl -n <namespace> delete configmap/<deployment>-otelcol secret/<deployment>-otelcol-headers --ignore-not-found
```

For the same reason, a change to the plugin settings reaches a deployment that already has
the sidecar only after you remove the sidecar so that it is injected again. The ConfigMap
itself is updated on every save, and pods pick it up when they restart.

## Enable for an application

Provision the **OpenTelemetry Collector** resource (id `tracing`) on the application:

- **UI:** Application → **Infrastructure** tab → **Add OpenTelemetry Collector**.
- **MCP:** `provision_resource` with `plugin_name: otel`, `resource_id: tracing`.
- **REST:** `POST /api/namespaces/{ns}/applications/{app}/resources/otel/tracing`.

deckwatch saves the result and applies it to every deployment **and cronjob** labelled
`deckwatch.io/application=<app>`. It reconciles about every 10 seconds, and repeating the
same patch is a no-op. Every deployment in the application is annotated with
`deckwatch.io/otel-application: <app>`, and deployment-level injection leaves those
deployments alone.

| Field | Description |
|---|---|
| `OTEL_SERVICE_NAME` | Optional: one service name for every workload. Leave it empty so each deployment keeps its own. deckwatch's trace view looks traces up by deployment name. |
| `OTEL_RESOURCE_ATTRIBUTES` | Optional extra `key=value` pairs (%-encoded) appended after `k8s.namespace.name` and `deckwatch.application.name`. |

After you change the plugin settings, click **Refresh** on the resource (or use the
`refresh_resource` MCP tool). Refreshing renders the configuration again, updates the
ConfigMap, and changes the sidecar's `OTELCOL_CONFIG_SHA256`, which rolls the pods.

On deckwatch 0.6.x, application-level injection has limits that deployment-level injection
does not have:

- **Static values.** The reconciler applies one fixed set of env vars to every workload, so
  there is no per-deployment `OTEL_SERVICE_NAME` or `k8s.deployment.name`. The plugin
  therefore sets only what the pipelines need: the endpoint, the protocol, resource
  attributes for the namespace and application, and `OTEL_<SIGNAL>_EXPORTER=otlp` for the
  enabled signals.
- **Your same-named variables are replaced.** The reconciler uses server-side apply with
  force, so a variable you set with one of the names above is replaced.
- **CronJobs never finish.** The reconciler injects the sidecar into cronjob pods as an
  ordinary container, and the collector never exits, so their Jobs never complete. Do not
  enable the collector at application level for applications that have cronjobs.
- **Removal is incomplete.** If you remove the resource while the application has no other
  plugin resource, deckwatch stops reconciling the application and leaves the sidecar and
  env vars in place.

**Disable:** remove the resource (**Deprovision**, or the `deprovision_resource` MCP tool).
If the application still has other plugin resources, deckwatch removes the sidecar, its
volumes, the env vars and the annotation at its next reconcile. deckwatch never deletes
the `<app>-otelcol` ConfigMap or the `<app>-otelcol-headers` Secret; delete them once no
workload uses them.

`associate_plugin` (the application's **Plugins** list) has no effect on this plugin:
deckwatch 0.6.x's reconciler does not use plugin associations.

## Before and after

The following deployment-level example has `OTLP_HEADERS` set. Without headers, the
`otelcol-headers` volume, its mount and the Secret are absent.

Before:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: checkout
  namespace: shop
  annotations:
    deckwatch.io/otel: enabled
spec:
  template:
    spec:
      containers:
        - name: checkout
          image: registry.local/shop/checkout:1.4.2
          env:
            - name: LOG_LEVEL
              value: info
```

After deckwatch saves it:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: checkout
  namespace: shop
  annotations:
    deckwatch.io/otel: enabled
    deckwatch.plugin-env/otel: OTEL_EXPORTER_OTLP_ENDPOINT,OTEL_EXPORTER_OTLP_PROTOCOL,OTEL_SERVICE_NAME,OTEL_RESOURCE_ATTRIBUTES,OTEL_TRACES_EXPORTER,OTEL_METRICS_EXPORTER,OTEL_LOGS_EXPORTER
    deckwatch.plugin-sidecar/otel: otel-collector
spec:
  template:
    spec:
      containers:
        - name: checkout
          image: registry.local/shop/checkout:1.4.2
          env:
            - name: LOG_LEVEL
              value: info
            - name: OTEL_EXPORTER_OTLP_ENDPOINT
              value: http://localhost:4317
            - name: OTEL_EXPORTER_OTLP_PROTOCOL
              value: grpc
            - name: OTEL_SERVICE_NAME
              value: checkout
            - name: OTEL_RESOURCE_ATTRIBUTES
              value: k8s.namespace.name=shop,k8s.deployment.name=checkout
            - name: OTEL_TRACES_EXPORTER
              value: otlp
            - name: OTEL_METRICS_EXPORTER
              value: none
            - name: OTEL_LOGS_EXPORTER
              value: none
        - name: otel-collector
          image: otel/opentelemetry-collector-contrib:0.165.0@sha256:e40d61f10c9a617cca1ffd1be4d7e2b3a4b0f0001798ff07e58cabe82e618cea
          env:
            - name: OTELCOL_CONFIG_SHA256   # sha256 of config.yaml; rolls pods on change
              value: 92af4811f8a63fb4b28f229f7f790e4fe2e2acce01ba4bb1039f68af4b288b67
          resources:
            requests: { cpu: 100m, memory: 128Mi }
            limits: { cpu: 500m, memory: 256Mi }
          volumeMounts:
            - name: otelcol-config
              mountPath: /etc/otelcol-contrib
              readOnly: true
            - name: otelcol-headers
              mountPath: /etc/otelcol-secrets
              readOnly: true
      volumes:
        - name: otelcol-config
          configMap:
            name: checkout-otelcol
        - name: otelcol-headers
          secret:
            secretName: checkout-otelcol-headers
```

The sidecar declares no container ports, because nothing outside the pod can reach it.

The plugin also creates the ConfigMap `shop/checkout-otelcol` (key `config.yaml`) and the
Secret `shop/checkout-otelcol-headers` (one key per header). Both are labelled
`app.kubernetes.io/managed-by: deckwatch-plugin-otel`. The rendered configuration for gRPC
with one header:

```yaml
# Managed by deckwatch-plugin-otel. Changes are overwritten.
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: "127.0.0.1:4317"
      http:
        endpoint: "127.0.0.1:4318"
processors:
  memory_limiter:
    check_interval: "1s"
    limit_percentage: 80
    spike_limit_percentage: 25
  batch:
    timeout: "200ms"
    send_batch_size: 8192
exporters:
  otlp_grpc:
    endpoint: "tempo.observability.svc:4317"
    tls:
      insecure: true
    headers:
      "authorization": "${file:/etc/otelcol-secrets/authorization}"
service:
  telemetry:
    metrics:
      level: none
  pipelines:
    traces:
      receivers: [otlp]
      processors: [memory_limiter, batch]
      exporters: [otlp_grpc]
```

The collector's internal metrics are turned off (`level: none`). Otherwise they would listen
on `localhost:8888`, which could clash with an application port in the shared network
namespace.

## How this relates to deckwatch's tracing settings

deckwatch's `tracing` settings (see `docs/TRACING.md` in deckwatch) configure the trace
viewer, not this plugin:

- **`tracing.otlp_endpoint` / `tracing.otlp_insecure`** describe where your backend receives
  OTLP. Plugins cannot read deckwatch settings, so copy the same values into `OTLP_ENDPOINT`
  and `OTLP_INSECURE`.
- **`tracing.query_url`, `tracing.ui_url`, `tracing.backend_kind`** drive the deployment's
  **Traces** card. The card searches for `service.name` equal to the deployment name, which is
  why `OTEL_SERVICE_NAME` defaults to the deployment name.

## Security notes

- **Loopback only.** The receivers bind 127.0.0.1, and the sidecar declares no ports, so other
  pods cannot send telemetry through it. This was checked against the real collector image:
  the application container in the same network namespace got HTTP 200, and a different
  network namespace got a connection refused.
- **Secrets.** Header values exist only in the namespaced Secret, mounted read-only into the
  sidecar. Error messages never quote them, and their `Debug` output is redacted. A
  deckwatch-encrypted value is refused rather than sent to your backend.
- **Least privilege.** The plugin creates only a ConfigMap and a Secret, in the workload's
  namespace. It creates no cluster-scoped objects, no RBAC, no ServiceAccounts and no owner
  references, and the sidecar needs no Kubernetes API access.
- **No outbound HTTP.** Keep `allowed_hosts` empty. The module imports only extism's kernel
  functions (config, input, output, memory, errors).
- **Pinned image.** The default image is pinned by digest, and `:latest` and untagged images
  are rejected.
- **securityContext.** deckwatch does not yet let plugins set a container `securityContext`,
  so the pod spec cannot enforce `runAsNonRoot`, `readOnlyRootFilesystem`,
  `allowPrivilegeEscalation: false` and dropping all capabilities. The default image runs as
  UID 10001 from a `scratch` base with no file capabilities. It was verified to run unchanged
  with a read-only root filesystem, all capabilities dropped, no-new-privileges and UID 10001,
  so those settings can be enforced as soon as deckwatch supports them.

## Upgrading from 0.1.0

0.1.0 used `otel/opentelemetry-collector:latest` and the image's default configuration, which
listens on 0.0.0.0 and has no real exporter.

- The settings keys have changed: `OTEL_IMAGE` is ignored, and `OTLP_ENDPOINT` must be set.
- The application resource id is still `tracing`. Click **Refresh** on existing resources to
  replace the old sidecar.
- The old resource field `OTEL_IMAGE` is ignored, and `OTEL_SERVICE_NAME` keeps its meaning.

## Building and testing

```sh
rustup target add wasm32-unknown-unknown
make test    # unit tests on the host target
make lint    # rustfmt, clippy for the host and wasm32
make build   # dist/plugin.wasm and dist/plugin.wasm.sha256
```

The tests check the pod spec after deckwatch merges the plugin's output (`src/host_sim.rs`
mirrors deckwatch's merge), that repeated saves change nothing, that enabling and then
disabling restores the original deployment, the rendered collector configuration, the
handling of secrets, and that every output deserializes into deckwatch's own plugin types.

**Releases:** bump `version` in `Cargo.toml` and push a matching `v*` tag. The release
workflow runs the tests, builds `plugin.wasm`, and attaches it together with
`plugin.wasm.sha256`. The release notes include the checksum to pin in deckwatch.

## Marketplace

To list a release in the [deckwatch marketplace](http://market.deckwatch.io), add or update
this entry in `catalog.json` of
[alexconrey/deckwatch-marketplace](https://github.com/alexconrey/deckwatch-marketplace):

```json
{
  "name": "OpenTelemetry Collector",
  "slug": "deckwatch-plugin-otel",
  "description": "Adds an OpenTelemetry Collector sidecar to deployments or whole applications and wires the OTEL_* environment.",
  "author": "alexconrey",
  "homepage": "https://github.com/alexconrey/deckwatch-plugin-otel",
  "trust_level": "community",
  "tags": ["otel", "opentelemetry", "tracing", "observability", "sidecar"],
  "latest_version": "0.2.0",
  "source": {
    "type": "github",
    "repo": "alexconrey/deckwatch-plugin-otel",
    "ref": "v0.2.0",
    "path": "plugin.wasm",
    "use_release": true
  },
  "allowed_hosts_hint": []
}
```

## License

Apache-2.0
