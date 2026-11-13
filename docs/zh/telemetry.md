[English](../en/telemetry.md) · [日本語](../ja/telemetry.md) · [简体中文](../zh/telemetry.md) · [文档索引](README.md)

<a id="opentelemetry"></a>
# OpenTelemetry

受 [TAKT 可观测性支持](https://github.com/nrslib/takt/blob/main/docs/observability.md) 启发，Cruise 可以通过 OTLP HTTP/protobuf 导出 workflow traces、执行 metrics，以及长度受限的失败命令 stderr 日志。遥测默认关闭；无需更改 workflow YAML。

```sh
CRUISE_OBSERVABILITY_ENABLED=true \
OTEL_SERVICE_NAME=cruise \
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf \
cruise exec --config cruise.yaml
```

- 设置 `CRUISE_OBSERVABILITY_ENABLED=true` 即启用（不区分大小写）。`OTEL_SDK_DISABLED=true` 会覆盖该设置。遥测关闭时不会创建 exporter 或网络客户端。
- 未指定 endpoint 覆盖项时，启用遥测后使用 `http://localhost:4318`。系统会在基础 endpoint 后追加 `/v1/traces`、`/v1/metrics` 和 `/v1/logs`。`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`、`OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` 和 `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` 可分别原样覆盖各 URL（包括其中的信号路径）。
- `OTEL_EXPORTER_OTLP_HEADERS` 提供以逗号分隔、经过 URL 编码的 `key=value` 认证请求头；请使用环境变量/secret 管理器，不要放在 workflow YAML 中。`OTEL_EXPORTER_OTLP_TIMEOUT` 设置请求超时（毫秒；SDK 默认值为 10,000）。特定信号的 `OTEL_EXPORTER_OTLP_TRACES_*` / `OTEL_EXPORTER_OTLP_METRICS_*` / `OTEL_EXPORTER_OTLP_LOGS_*` 值优先级更高。
- 仅支持 HTTP/protobuf；选择 gRPC 或 HTTP/JSON 协议时会回退到 HTTP/protobuf。可选压缩方式：`OTEL_EXPORTER_OTLP_COMPRESSION=gzip`。
- 默认资源属性为 `service.name=cruise` 和软件包的 `service.version`。`OTEL_RESOURCE_ATTRIBUTES` 可添加或覆盖资源属性；对于服务名称，`OTEL_SERVICE_NAME` 优先级更高。

分配 session ID 后，`cruise.execution` spans 会覆盖从准备到最终收尾的全过程，包括 session 持久化之前的仓库克隆。CLI 的 plan/exec/run、后台规划 worker，以及 TUI/WebUI 共用的 Application API 均包含在内；并行会话各自拥有独立的 execution 根 span。根属性包括 `cruise.execution.kind` 和 `cruise.session.id`；若已有配置，则设置 `cruise.workflow.kind` 和 `cruise.workflow.name`。Workflow 名称为内置名称或文件的最终文件名，绝不包含完整路径。准备失败时，可能已有 trace 分配的 ID，但尚未持久化 session。

| Span | 范围 |
|---|---|
| `cruise.execution` | 一个 session 操作，通过 session ID 关联 |
| `cruise.phase` | 实际执行的 `prepare`、`workspace`、`planning`、`workflow`、`pull_request` 或 `finalize` 工作 |
| `cruise.workflow` | workflow 阶段下的引擎执行 |
| `cruise.step` | 单个步骤；并行子步骤是其并行步骤的同级 span |
| `cruise.agent` | 一次 prompt 后端调用，包括其重试生命周期 |
| `cruise.attempt` | 一次后端或 shell 尝试，包含从 1 开始的 `cruise.retry.attempt` 和所选后端/模型 |

只有实际发生的阶段才会导出：跳过的规划或已禁用/不需要的 PR 创建不会生成表示成功的占位阶段。`cruise.retry` span events 会记录退避/回退操作、固定重试原因、尝试次数和模型切换。SDK 回退以及独立的命令/prompt 速率限制重试循环都会被纳入监测，但不会改变重试策略。

如果 GitHub 原生 squash 合并因无法确认可合并性而被拒绝，即使遥测关闭，保存的 session 失败信息也会包含观察到的 `mergeable` 和 `mergeStateStatus` 值。启用遥测时，`squash_merge` 步骤 span 还会将这些值记录为 `cruise.github.mergeable` 和 `cruise.github.merge_state_status` 属性。

同一 span 还会包含最终的 GitHub 就绪状态观察结果：

| 属性 | 值 |
|---|---|
| `cruise.github.waiting_for` | `CI registration`、`current-head CI registration`、`current-head CI checks` 或 `GitHub mergeability`；若未观察到待处理原因或就绪状态已解决，则为 `none` |
| `cruise.github.ci_state` | 观察到的 `statusCheckRollup.state`；快照之前为 `unobserved`，缺失时为 `unregistered` |
| `cruise.github.ci_head_matches` | 比较观察到的 CI 汇总提交与预期 HEAD 的布尔值；缺少 CI 提交或尚未观察到时同样为 `false` |

最新观察值会保存在本地，并在合并操作返回或其 future 被丢弃时写入一次，包括外层步骤超时和取消的情况。这可避免重复轮询属性耗尽 SDK 的 span 属性数量上限。就绪或已合并结果会清除先前的等待原因；初始的 `none` / `unobserved` / `false` 值不能证明 CI 已通过。不会导出提交 ID。

如果 GitHub squash 合并命令失败结束，并确认 pull request 仍处于打开状态且 HEAD 未变，保存的失败信息会包含固定拒绝原因，以及可获取的 HTTP 状态和命令退出码。同一 span 会记录 `cruise.github.merge_refusal_reason`，并在可获取时记录数值型 `cruise.github.merge_http_status` 和 `cruise.github.merge_exit_code`。原因包括 `merge_not_performed`、`head_changed`、`base_changed`、`squash_disabled`、`merge_in_progress` 或 `unclassified`。最后一种表示响应不匹配任何已识别的规范消息，而非根据状态码推断原因。不会保存或导出原始 provider 消息和响应头。这些属性不是 metrics 标签。

被跳过的步骤（包括并行子步骤）不会生成步骤 span 或操作观察结果。步骤结果包括后置条件失败，例如 `if.no-file-changes`；后续恢复成功也不会抹去失败步骤的记录。取消审批菜单会记录为已取消，但仍保留成功的 CLI 退出结果；中断正在进行的规划或标题生成则会在清理该工作后返回错误。选择 Execute now 时，会先完成规划，再以单独的 execution 根 span 开始执行。

结果包括 `success`、`failed`、`error`、`cancelled`、`timeout` 和 `paused`。非成功操作会设置错误状态，并附上不含载荷的 `error.type`，例如 `command_exit`、`rate_limit`、`timeout` 或 `cancelled`；span 状态描述与 `error.type` 相同。现有后端重试分类会区分网络、provider 和模型故障。恢复成功的重试仍会保留失败尝试记录，但不会将最终成功的执行标记为失败。按 Esc 暂停的 exec 仍可恢复，CLI 会成功退出，而其根结果仍为 `paused`。

失败的 shell 尝试包含数值型 `cruise.command.exit_code`，或在 Unix 上被信号终止时包含 `cruise.command.signal`。最终失败结果也会为其步骤 span 添加注释；恢复成功的速率限制尝试不会注释成功步骤。

已完成但失败的 shell 结果还会在这些 spans 上记录 `cruise.command.stderr_present`（捕获的 stderr 是否非空，包括仅含空白字符的情况）和 `cruise.ci.diagnostic_status`：

| 诊断状态 | 含义 |
|---|---|
| `valid` | 接受了受长度限制且字段在允许列表中的 `CRUISE_CI_DIAGNOSTIC_V2` 或 `CRUISE_CI_DIAGNOSTIC_V4` 标记 |
| `invalid` | 存在 V2 或 V4 标记候选，但没有任何一个通过验证 |
| `unsupported` | 只识别到不支持的数字版本标记 |
| `missing` | 捕获的 stderr 中没有识别到 CI 标记候选 |

即使之后的行格式错误或版本不受支持，最后一个有效 V2 或 V4 标记仍优先；否则 `invalid` 优先于 `unsupported`。候选内容必须以标记 token 开始；检测时允许其前面有 ASCII 空白。两个版本都限制为 16,384 bytes，且只允许标量字段；V2 不接受尾随字段，V4 则要求严格验证的 JSON 详情载荷。未知版本、未知 JSON 字段和格式错误的值绝不导出；无效的可选引用会被省略，同时保留有效的检查名称/状态并标记详情不完整。`missing` 也可能表示无关命令或较旧 workflow 未包含诊断标记：这些状态描述的是捕获到的输出，而非底层 CI 故障。对于未完成 shell 结果的启动错误、超时和取消，不会添加这些字段。这两个属性仅用于 traces，不是 metrics 标签。

Jcode 故障会在受影响的 `cruise.attempt` span 上添加固定诊断：

| 属性 | 含义 |
|---|---|
| `cruise.agent.failure_stage` | 检测到故障的有限阶段，例如 SDK 启动、API 事件处理或 turn 停止处理 |
| `cruise.agent.failure_reason` | 针对有类型的 SDK/API/turn 故障所使用的固定允许原因；未知情况保留为 `unknown`，而不会推断 HTTP 状态、认证或 provider 内部原因 |

这些属性仅用于 traces，不是 metrics 标签。不会导出原始 SDK 错误消息、停止消息、prompts 或输出。

内置的 `follow-up/wait-ci` 命令还会报告受长度限制且字段在允许列表中的诊断：

| 属性 | 含义 |
|---|---|
| `cruise.ci.provider` | `github` 或 `gitlab` |
| `cruise.ci.failure_reason` | 固定原因：`local_head_error`、`repository_query_error`、`repository_invalid`、`api_error`、`graphql_error`、`pr_head_missing`、`head_mismatch`、`rollup_head_missing`、`malformed_rollup`、`invalid_ci_state`、`invalid_check_count`、`ci_failed`、`unknown_state`、`clock_error` 或 `timeout` |
| `cruise.ci.state` | 最后观察到的允许 CI 状态、注册状态，或 `unknown` / `unobserved`；绝不包含任意 provider 文本 |
| `cruise.ci.head_matches` | 观察到的 GitHub PR HEAD 是否与本地 HEAD 匹配；未观察到时省略。PR HEAD 匹配并不能证明汇总信息是最新的 |
| `cruise.ci.check_count` | 可获取时记录观察到的数值检查数量 |
| `cruise.ci.http_status` | 观察到的 100 到 599 之间的数值 HTTP 状态；未知时省略 |
| `cruise.ci.failed_checks` | 对于 `ci_failed`，是一个紧凑 JSON 数组，包含失败检查/job 描述项，其中有 `name`、`state` 以及可获取的 `run_id`、`job_id` 和 `url` 引用 |
| `cruise.ci.failed_checks_truncated` | 对于 `ci_failed`，表示详情不完整、无效引用/条目已省略，或名称/引用已规范化/缩短 |

失败详情最多包含 10 项检查；名称在移除控制字符并裁去首尾空白后，最多为 128 个 Unicode 字符。GitHub 会收集已完成且结论失败的 CheckRun，以及状态失败的 StatusContext；只有汇总结果失败后，才会获取并分页读取详情响应。若可获取 workflow run ID，CheckRun 引用会包含该 ID；只有当详情 URL 中的仓库和 run ID 与观察到的仓库/workflow run 相匹配时，才会提取 Actions job ID。StatusContext 条目不会虚构 run/job ID。GitLab 只有在详情响应与观察到的失败 pipeline 匹配时，才会使用其 pipeline ID、失败 job ID、URL 或获取本地 trace。引用 ID 必须是最多 20 位的正十进制字符串。URL 必须使用 HTTP(S)，且最多 1,024 bytes；其中 userinfo、query 和 fragment 会被移除。详情缺失或无效时仍保留 `ci_failed`，并设置截断标记；若有必要，生产端会丢弃可选引用，以确保完整标记不超出大小限制。这些描述项仅用于 traces，绝不作为 metrics 标签。完整 CI job 日志仍保留在 provider 端；遥测只记录引用，不会复制日志。

内置的 `follow-up/sync-base` 命令会导出 `cruise.sync_base.failure_reason`，其值为 `base_ref_query_error`、`base_ref_invalid`、`fetch_failed`、`merge_conflict` 或 `merge_failed`。CI 和 sync-base span 诊断来自严格、长度受限的 stderr 标记；格式错误的标记字段和原始 provider 响应不会附加到 spans。所有 CI/sync-base 机器标记行（包括无效或不受支持的标记）都会从 stderr 日志中省略。已保存的 workflow 快照和此前导出的 workflow 会保留旧版 CI 命令。无需替换这些命令也能读取 V2 诊断；对于 `ci_failed`，若无法获取检查描述项，则导出空数组并设置 `cruise.ci.failed_checks_truncated=true`。V3 不受支持。创建新快照或更新已导出的 workflow，才能取得 V4 检查详情；现有导出 spans 不会改变。

归类为 `loop_protection` 的错误会在每个基于结果的外层 span 上附加 `cruise.loop.from`、`cruise.loop.to`、`cruise.loop.max_retries`、`cruise.loop.traversals`（该边在被拒绝的转移之前，已接受的预算内遍历次数）和 `cruise.loop.skipped_step_count`。步骤 ID 遵循既有步骤名称隐私策略；不会导出完整边表、跳过步骤列表或异常消息。重试预算和 workflow 行为均不变。

Metrics 包括 `cruise.operation.count` 和 `cruise.operation.duration`（秒），标签仅为 `cruise.operation` 和 `cruise.outcome`；以及 `cruise.retry.count`，标签仅为 `cruise.retry.action` 和 `cruise.retry.reason`。Session/workflow/model 标识只用于 traces，不是 metrics 标签。不会向 OpenTelemetry 导出 token 用量和成本；这些信息会像 [`cruise usage`](cli-reference.md#cruise-usage) 中所述记录在本地。

Prompt/input 文本、代理输出、命令正文、工作路径和异常消息不会自动附加到 spans 或 metrics。Session ID、workflow 文件名、步骤/模型名称、CI 检查/job 名称和引用，以及显式提供的资源属性会被导出；请勿在这些值中放入秘密信息。

每次已完成但失败的 shell 尝试还可以额外生成一条名为 `cruise.command.stderr` 的 ERROR OTLP 日志，并通过该尝试原生的 `trace_id` 和 `span_id` 进行关联。最终步骤不会重复生成该日志；成功命令的 stderr 不会导出；经过清理的输出为空或仅含空白时不会生成记录。没有已完成 shell 结果的启动错误、超时和取消不会生成 stderr 日志。

日志正文会删改已识别的凭据请求头和赋值、认证方案、已知 token 格式、私钥块以及 URL userinfo/query/fragment 值；匹配前会规范化 ANSI/控制序列。删改发生在截取 UTF-8 安全的首尾片段之前，截取上限为 8,192 bytes。`cruise.command.stderr_truncated` 表示清理后输出是否超过该上限，而非是否删改了凭据。

删改基于模式匹配，不能保证移除任意秘密或个人数据。捕获的 stderr 仍可能包含敏感路径、消息或业务数据。在真实工作负载中启用遥测前，请将后端日志访问权限限制为经授权的操作人员，并设置较短的保留期限。Cruise 不会配置后端 RBAC 或保留策略；这些仍由部署方负责，启用日志不会更改它们。

CLI 正常结束时（包括 workflow 出错）会刷新 provider。Collector/export 失败不会改变 workflow 结果，但关闭过程可能等待导出超时。无效的 exporter 初始化设置会以经过删改的诊断信息阻止启动。强制终止无法保证完成刷新。

后端接受请求并不保证数据已持久化。本地 E2E 调查发现：SDK 分别发送导出请求时，GreptimeDB 保留了所有 histogram observation；但当多个 `ResourceMetrics` 合并到同一个 OTLP 请求时，它丢失了较早的 histogram 数据点。SDK payload 中包含了所有 observations，而且 gateway 返回了 HTTP 200。应用不会通过重复导出规避这一下游摄取问题。
