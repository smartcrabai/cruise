<a id="telemetry-guide"></a>
# Telemetry guide

**Languages:** [English](../en/telemetry.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/telemetry.md) · [简体中文](../zh/telemetry.md) · [Index](README.md)

<a id="opentelemetry"></a>
## OpenTelemetry

Cruise can export workflow traces, execution metrics, and bounded failed-command stderr logs over OTLP HTTP/protobuf. Telemetry is disabled by default; no workflow YAML changes are needed.

```sh
CRUISE_OBSERVABILITY_ENABLED=true \
OTEL_SERVICE_NAME=cruise \
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf \
cruise exec --config cruise.yaml
```

- `CRUISE_OBSERVABILITY_ENABLED=true` opts in (case-insensitive). `OTEL_SDK_DISABLED=true` takes precedence.
- Without an endpoint override, enabled telemetry uses `http://localhost:4318`. The base endpoint gets `/v1/traces`, `/v1/metrics`, and `/v1/logs` appended. `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`, `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`, and `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` override each URL verbatim, including its signal path.
- `OTEL_EXPORTER_OTLP_HEADERS` supplies comma-separated, URL-encoded `key=value` authentication headers; use an environment variable or secret manager, not workflow YAML. `OTEL_EXPORTER_OTLP_TIMEOUT` sets the request timeout in milliseconds. Signal-specific `OTEL_EXPORTER_OTLP_TRACES_*` / `OTEL_EXPORTER_OTLP_METRICS_*` / `OTEL_EXPORTER_OTLP_LOGS_*` values take precedence.
- Only HTTP/protobuf is supported; gRPC and HTTP/JSON protocol selections fall back to HTTP/protobuf. Optional compression: `OTEL_EXPORTER_OTLP_COMPRESSION=gzip`.
- Resources default to `service.name=cruise` and the package's `service.version`. `OTEL_RESOURCE_ATTRIBUTES` adds or overrides resource attributes; `OTEL_SERVICE_NAME` takes precedence for the service name.

Metrics are `cruise.operation.count` and `cruise.operation.duration` (seconds), labeled only by `cruise.operation` and `cruise.outcome`, plus `cruise.retry.count`, labeled only by `cruise.retry.action` and `cruise.retry.reason`. Session/workflow/model identifiers are trace-only, not metric labels. Token usage and cost are not exported to OpenTelemetry; they are recorded locally as described under `cruise usage`.

Prompt/input text, agent output, command bodies, working paths, and exception messages are not automatically attached to spans or metrics. Session IDs, workflow file names, step/model names, CI check/job names and references, and explicitly supplied resource attributes are exported; keep secrets out of those values.

A completed failed shell attempt may additionally emit an ERROR OTLP log named `cruise.command.stderr`. Log bodies redact credential headers, known token formats, private-key blocks, and URL userinfo/query values, and are capped at 8,192 bytes. Stderr from successful commands is not exported.

Redaction is pattern-based, not a guarantee that arbitrary secrets or personal data are removed. Captured stderr can still contain sensitive paths, messages, or business data. Before enabling telemetry for real workloads, restrict backend log access to authorized operators and configure short retention. Cruise does not provision backend RBAC or retention policies; these remain deployment responsibilities and are not changed by enabling logs.

Providers flush on normal CLI completion, including workflow errors. Collector/export failures do not change the workflow result.
