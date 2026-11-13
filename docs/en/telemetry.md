# Telemetry guide

**Languages:** [English](../en/telemetry.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/telemetry.md) · [简体中文](../zh/telemetry.md) · [Index](README.md)

## OpenTelemetry

Inspired by [TAKT's observability support](https://github.com/nrslib/takt/blob/main/docs/observability.md), Cruise can export workflow traces, execution metrics, and bounded failed-command stderr logs over OTLP HTTP/protobuf. Telemetry is off by default; no workflow YAML changes are needed.

```sh
CRUISE_OBSERVABILITY_ENABLED=true \
OTEL_SERVICE_NAME=cruise \
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf \
cruise exec --config cruise.yaml
```

- `CRUISE_OBSERVABILITY_ENABLED=true` opts in (case-insensitive). `OTEL_SDK_DISABLED=true` overrides it. Disabled telemetry creates no exporter or network client.
- Without an endpoint override, enabled telemetry uses `http://localhost:4318`. The base endpoint gets `/v1/traces`, `/v1/metrics`, and `/v1/logs` appended. `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`, `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`, and `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` override each URL verbatim, including its signal path.
- `OTEL_EXPORTER_OTLP_HEADERS` supplies comma-separated, URL-encoded `key=value` authentication headers; use an environment/secret manager, not workflow YAML. `OTEL_EXPORTER_OTLP_TIMEOUT` sets the request timeout in milliseconds (SDK default: 10,000). Signal-specific `OTEL_EXPORTER_OTLP_TRACES_*` / `OTEL_EXPORTER_OTLP_METRICS_*` / `OTEL_EXPORTER_OTLP_LOGS_*` values take precedence.
- Only HTTP/protobuf is supported; gRPC and HTTP/JSON protocol selections fall back to HTTP/protobuf. Optional compression: `OTEL_EXPORTER_OTLP_COMPRESSION=gzip`.
- Resources default to `service.name=cruise` and the package's `service.version`. `OTEL_RESOURCE_ATTRIBUTES` adds or overrides resource attributes; `OTEL_SERVICE_NAME` takes precedence for the service name.

Once a session ID is allocated, `cruise.execution` spans preparation through finalization, including repository cloning before session persistence. CLI plan/exec/run, background planning workers, and the shared Application API used by TUI/WebUI are covered; parallel sessions have separate execution roots. Root attributes include `cruise.execution.kind` and `cruise.session.id`; `cruise.workflow.kind` and `cruise.workflow.name` are set when configuration is available. Workflow names are built-in names or final file names, never full paths. A preparation failure can leave a trace with an allocated ID but no persisted session.

| Span | Scope |
|---|---|
| `cruise.execution` | A session operation, correlated by session ID |
| `cruise.phase` | Actual `prepare`, `workspace`, `planning`, `workflow`, `pull_request`, or `finalize` work |
| `cruise.workflow` | Engine execution under the workflow phase |
| `cruise.step` | One step; parallel children are siblings under their parallel step |
| `cruise.agent` | A prompt backend call, including its retry lifecycle |
| `cruise.attempt` | One backend or shell attempt, with a 1-based `cruise.retry.attempt` and the selected backend/model |

Phases are emitted only when their work runs: skipped planning or disabled/unneeded PR creation does not produce a successful placeholder phase. `cruise.retry` span events record backoff/fallback actions, fixed retry reasons, attempt numbers, and model switches. SDK fallback and the independent command/prompt rate-limit retry loops are instrumented without changing retry policy.

GitHub native squash-merge refusals for unconfirmed mergeability include the observed `mergeable` and `mergeStateStatus` values in the saved session failure, even when telemetry is disabled. When enabled, telemetry also records these values as `cruise.github.mergeable` and `cruise.github.merge_state_status` attributes on the `squash_merge` step span.

The same span includes the final GitHub readiness observation:

| Attribute | Value |
|---|---|
| `cruise.github.waiting_for` | `CI registration`, `current-head CI registration`, `current-head CI checks`, or `GitHub mergeability`; `none` when no pending reason was observed or readiness resolved |
| `cruise.github.ci_state` | Observed `statusCheckRollup.state`; `unobserved` before a snapshot, or `unregistered` when absent |
| `cruise.github.ci_head_matches` | Boolean comparing the observed CI rollup commit to the expected HEAD; `false` also covers a missing or not-yet-observed CI commit |

The latest observation is retained locally and written once when the merge operation returns or its future is dropped, including outer step timeouts and cancellation. This avoids duplicate polling attributes exhausting the SDK's span attribute limit. A ready or already-merged result clears a previous wait reason; initial `none` / `unobserved` / `false` values do not prove that CI passed. Commit IDs are not exported.

When a GitHub squash merge command completes unsuccessfully and the pull request is confirmed still open at the expected head, the saved failure includes a fixed refusal reason and any available HTTP status and command exit code. The same span records `cruise.github.merge_refusal_reason` and, when available, numeric `cruise.github.merge_http_status` and `cruise.github.merge_exit_code`. Reasons are `merge_not_performed`, `head_changed`, `base_changed`, `squash_disabled`, `merge_in_progress`, or `unclassified`. The last means the response did not match a recognized canonical message, not that a cause was inferred from the status. Raw provider messages and response headers are not saved or exported. These attributes are not metric labels.

Skipped steps, including parallel children, do not emit step spans or operation observations. Step outcomes include postcondition failures such as `if.no-file-changes`; a later recovery can succeed without erasing the failed step. Cancelling the approval menu is recorded as cancelled while preserving the successful CLI exit; interrupting active planning or title generation still returns an error after draining that work. Choosing Execute now finishes planning before starting a separate execution root.

Outcomes include `success`, `failed`, `error`, `cancelled`, `timeout`, and `paused`. Non-success operations have error status and a payload-free `error.type`, such as `command_exit`, `rate_limit`, `timeout`, or `cancelled`; the span status description equals `error.type`. Existing backend retry classifications distinguish network, provider, and model failures. Recovered retries leave failed attempts visible without marking the final successful execution as failed. An Esc-paused exec stays resumable and exits successfully at the CLI, while its root outcome remains `paused`.

Failed shell attempts include numeric `cruise.command.exit_code` or, on Unix when terminated by a signal, `cruise.command.signal`. The terminal failed result also annotates its step span; a recovered rate-limit attempt does not annotate the successful step.

Completed unsuccessful shell results also record `cruise.command.stderr_present` (whether captured stderr is nonempty, including whitespace) and `cruise.ci.diagnostic_status` on those spans:

| Diagnostic status | Meaning |
|---|---|
| `valid` | A bounded, allowlisted `CRUISE_CI_DIAGNOSTIC_V2` or `CRUISE_CI_DIAGNOSTIC_V4` marker was accepted |
| `invalid` | A V2 or V4 marker candidate was present, but none passed validation |
| `unsupported` | Only unsupported numeric marker versions were recognized |
| `missing` | No CI marker candidate was recognized in captured stderr |

The last valid V2 or V4 marker wins even if later lines are malformed or unsupported; otherwise `invalid` takes precedence over `unsupported`. Candidates start with the marker token, allowing leading ASCII whitespace for detection only. Both versions are limited to 16,384 bytes and contain allowlisted scalar fields; V2 accepts no trailing fields, while V4 requires a strictly validated JSON detail payload. Unknown versions, unknown JSON fields, and malformed values are never exported; invalid optional references are omitted while retaining valid check names/states and marking details incomplete. `missing` can also describe an unrelated command or an old workflow without diagnostic markers: these statuses describe captured output, not the underlying CI failure. Spawn errors, timeouts, and cancellation without a completed shell result do not add these fields. Both attributes are trace-only, not metric labels.

Jcode failures add fixed diagnostics to the affected `cruise.attempt` span:

| Attribute | Meaning |
|---|---|
| `cruise.agent.failure_stage` | The bounded stage where the failure was detected, such as SDK launch, API event handling, or turn-stop handling |
| `cruise.agent.failure_reason` | A fixed allowlisted reason for typed SDK/API/turn failures; unknown cases remain `unknown` rather than inferring HTTP status, authentication, or provider-internal causes |

These attributes are trace-only, not metric labels. Raw SDK error messages, stop messages, prompts, and output are not exported.

The built-in `follow-up/wait-ci` command additionally reports bounded, allowlisted diagnostics:

| Attribute | Meaning |
|---|---|
| `cruise.ci.provider` | `github` or `gitlab` |
| `cruise.ci.failure_reason` | Fixed cause: `local_head_error`, `repository_query_error`, `repository_invalid`, `api_error`, `graphql_error`, `pr_head_missing`, `head_mismatch`, `rollup_head_missing`, `malformed_rollup`, `invalid_ci_state`, `invalid_check_count`, `ci_failed`, `unknown_state`, `clock_error`, or `timeout` |
| `cruise.ci.state` | Last observed allowlisted CI state, a registration state, or `unknown` / `unobserved`; never arbitrary provider text |
| `cruise.ci.head_matches` | Whether the observed GitHub PR head matches local HEAD; omitted when unobserved. A matching PR head does not prove the rollup is current |
| `cruise.ci.check_count` | Observed numeric check count when available |
| `cruise.ci.http_status` | Numeric HTTP status from 100 through 599 when observed; omitted when unknown |
| `cruise.ci.failed_checks` | For `ci_failed`, a compact JSON array of failed check/job descriptors containing `name`, `state`, and available `run_id`, `job_id`, and `url` references |
| `cruise.ci.failed_checks_truncated` | For `ci_failed`, whether details are incomplete, invalid references/entries were omitted, or names/references were normalized/shortened |

Failure details include at most 10 checks, with names limited to 128 Unicode characters after removing controls and trimming whitespace. GitHub collects completed failing CheckRun conclusions and failing StatusContext states, fetching and paginating detail responses only after a failed rollup. CheckRun references include the workflow run ID when available; an Actions job ID is extracted only when the details URL's repository and run ID match the observed repository/workflow run. StatusContext entries do not invent run/job IDs. GitLab requires the detail response to match the observed failed pipeline before using its pipeline ID, failed job IDs, URLs, or local trace fetching. Reference IDs are positive decimal strings of at most 20 digits. URLs are HTTP(S), at most 1,024 bytes, with userinfo, query strings, and fragments removed. Missing or invalid details preserve `ci_failed` and set the truncation flag; the producer drops optional references if needed to keep the complete marker bounded. These descriptors are trace-only, never metric labels. Full CI job logs remain on the provider; telemetry records references, not copies of those logs.

The built-in `follow-up/sync-base` command exports `cruise.sync_base.failure_reason` with one of `base_ref_query_error`, `base_ref_invalid`, `fetch_failed`, `merge_conflict`, or `merge_failed`. CI and sync-base span diagnostics come from strict bounded stderr markers; malformed marker fields and raw provider responses are not attached to spans. All CI/sync-base machine-marker lines, including invalid or unsupported ones, are omitted from stderr logs. Saved workflow snapshots and previously ejected workflows retain their old CI command. V2 diagnostics remain readable without replacing those commands; for `ci_failed`, unavailable check descriptors are exported as an empty array with `cruise.ci.failed_checks_truncated=true`. V3 remains unsupported. Create a new snapshot or update an ejected workflow to obtain V4 check details; existing exported spans are not changed.

Errors classified as `loop_protection` annotate each result-based enclosing span with `cruise.loop.from`, `cruise.loop.to`, `cruise.loop.max_retries`, `cruise.loop.traversals` (accepted budgeted traversals of that edge, before the refused transition), and `cruise.loop.skipped_step_count`. Step IDs follow the existing step-name privacy policy; full edge tables, skipped-step lists, and exception messages are not exported. The retry budget and workflow behavior are unchanged.

Metrics are `cruise.operation.count` and `cruise.operation.duration` (seconds), labeled only by `cruise.operation` and `cruise.outcome`, plus `cruise.retry.count`, labeled only by `cruise.retry.action` and `cruise.retry.reason`. Session/workflow/model identifiers are trace-only, not metric labels. Token usage and cost are not exported to OpenTelemetry; they are recorded locally as described under `cruise usage`.

Prompt/input text, agent output, command bodies, working paths, and exception messages are not automatically attached to spans or metrics. Session IDs, workflow file names, step/model names, CI check/job names and references, and explicitly supplied resource attributes are exported; keep secrets out of those values.

Each completed unsuccessful shell attempt can additionally emit one ERROR OTLP log named `cruise.command.stderr`, correlated to that attempt's native `trace_id` and `span_id`. The final step does not duplicate it, successful-command stderr is not exported, and empty/whitespace-only sanitized output emits no record. Spawn errors, timeouts, and cancellation without a completed shell result do not produce stderr logs.

Log bodies redact recognized credential headers and assignments, auth schemes, known token formats, private-key blocks, and URL userinfo/query/fragment values; ANSI/control sequences are normalized before matching. Redaction happens before a UTF-8-safe head/tail excerpt capped at 8,192 bytes. `cruise.command.stderr_truncated` records whether sanitized output exceeded that cap, not whether credentials were redacted.

Redaction is pattern-based, not a guarantee that arbitrary secrets or personal data are removed. Captured stderr can still contain sensitive paths, messages, or business data. Before enabling telemetry for real workloads, restrict backend log access to authorized operators and configure short retention. Cruise does not provision backend RBAC or retention policies; these remain deployment responsibilities and are not changed by enabling logs.

Providers flush on normal CLI completion, including workflow errors. Collector/export failures do not change the workflow result, though shutdown can wait for export timeouts. Invalid exporter initialization settings stop startup with a redacted diagnostic. Forced termination cannot guarantee a flush.

Backend acceptance is not a persistence guarantee. Local E2E investigation found that GreptimeDB retained all histogram observations when SDK exports were posted separately, but lost earlier histogram points when the same exports were combined into one OTLP request with multiple `ResourceMetrics`. The SDK payloads contained all observations and the gateway returned HTTP 200. This downstream ingestion issue is not worked around by duplicating application exports.
