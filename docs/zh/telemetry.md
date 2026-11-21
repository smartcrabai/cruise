<a id="telemetry-guide"></a>
# 遥测指南

[English](../en/telemetry.md) · [日本語](../ja/telemetry.md) · [简体中文](../zh/telemetry.md) · [文档索引](README.md)

<a id="opentelemetry"></a>
## OpenTelemetry

Cruise 可以通过 OTLP HTTP/protobuf 导出 workflow traces、执行 metrics，以及长度受限的失败命令 stderr 日志。遥测默认关闭；无需更改 workflow YAML。

```sh
CRUISE_OBSERVABILITY_ENABLED=true \
OTEL_SERVICE_NAME=cruise \
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf \
cruise exec --config cruise.yaml
```

- 设置 `CRUISE_OBSERVABILITY_ENABLED=true` 即启用（不区分大小写）。`OTEL_SDK_DISABLED=true` 会覆盖该设置。
- 未指定 endpoint 覆盖项时，启用遥测后使用 `http://localhost:4318`。系统会在基础 endpoint 后追加 `/v1/traces`、`/v1/metrics` 和 `/v1/logs`。`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`、`OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` 和 `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` 可分别原样覆盖各 URL（包括其中的信号路径）。
- `OTEL_EXPORTER_OTLP_HEADERS` 提供以逗号分隔、经过 URL 编码的 `key=value` 认证请求头；请使用环境变量或 secret 管理器，不要放在 workflow YAML 中。`OTEL_EXPORTER_OTLP_TIMEOUT` 设置请求超时（毫秒）。特定信号的 `OTEL_EXPORTER_OTLP_TRACES_*` / `OTEL_EXPORTER_OTLP_METRICS_*` / `OTEL_EXPORTER_OTLP_LOGS_*` 值优先级更高。
- 仅支持 HTTP/protobuf；选择 gRPC 或 HTTP/JSON 协议时会回退到 HTTP/protobuf。可选压缩方式：`OTEL_EXPORTER_OTLP_COMPRESSION=gzip`。
- 默认资源属性为 `service.name=cruise` 和软件包的 `service.version`。`OTEL_RESOURCE_ATTRIBUTES` 可添加或覆盖资源属性；对于服务名称，`OTEL_SERVICE_NAME` 优先级更高。

Metrics 包括 `cruise.operation.count` 和 `cruise.operation.duration`（秒），标签仅为 `cruise.operation` 和 `cruise.outcome`；以及 `cruise.retry.count`，标签仅为 `cruise.retry.action` 和 `cruise.retry.reason`。Session/workflow/model 标识只用于 traces，不是 metrics 标签。不会向 OpenTelemetry 导出 token 用量和成本；这些信息会像 `cruise usage` 中所述记录在本地。
Span 状态遵循操作结果：`success` 为 `Ok`；`paused` 和 `cancelled` 保持 `Unset`（结果记录在 `cruise.outcome` 中，不设置 `error.type`）；失败和超时为 `Error` 并设置 `error.type`。

Prompt/input 文本、代理输出、命令正文和异常消息不会自动附加到 spans 或 metrics。Session ID、workflow 文件名、步骤/模型名称、CI 检查/job 名称和引用，以及显式提供的资源属性会被导出；请勿在这些值中放入秘密信息。

Cruise 只会将完整路径作为字符串属性 `cruise.session.base_dir` 和 `cruise.session.workspace_dir` 导出到 `cruise.execution` trace span，不会添加到 child spans、metrics 或 logs。`base_dir` 是 session 的基础目录，不一定是 Git 根目录；使用 `--repo` 时则是 session 的托管克隆。session 加载并可供执行使用后会包含该属性。`workspace_dir` 是实际 workspace 目录，只有解析完成后才会包含，因此解析后的失败可能同时包含这两个属性。没有 session 可用的错误不会包含其中任何属性。路径未经删改，会原样导出。

这些原样导出的路径可能包含用户名、项目名称及其他可识别身份的信息。仅当 collector 及其存储可信，并且通过 RBAC 将访问限制给经授权的操作人员时，才启用此项。Cruise 不会配置这两者。

已完成但失败的 shell 尝试可能会额外生成一条名为 `cruise.command.stderr` 的 ERROR OTLP 日志。日志正文会删改凭据请求头、已知 token 格式、私钥块和 URL userinfo/query 值，并限制在 8,192 bytes 以内。成功命令的 stderr 不会导出。

删改基于模式匹配，不能保证移除任意秘密或个人数据。捕获的 stderr 仍可能包含敏感路径、消息或业务数据。在真实工作负载中启用遥测前，请将后端日志访问权限限制为经授权的操作人员，并设置较短的保留期限。Cruise 不会配置后端 RBAC 或保留策略；这些仍由部署方负责，启用日志不会更改它们。

会话工作在 daemon 中运行，因此 `CRUISE_OBSERVABILITY_ENABLED` 和 `OTEL_*` 变量由 daemon 进程读取：由命令自动启动的 daemon 继承该命令的值，已在运行的 daemon 继续使用其启动时的值。修改这些变量后，请在目标环境中运行 `cruise daemon restart`。daemon 会在每个操作结束时、客户端看到结果之前刷新 provider；CLI 命令在结束时（包括 workflow 出错）也会刷新其客户端侧遥测。Collector/export 失败不会改变 workflow 结果。
