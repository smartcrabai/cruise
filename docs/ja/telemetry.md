<a id="telemetry-guide"></a>
# テレメトリーガイド

**Languages:** [English](../en/telemetry.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/telemetry.md) · [简体中文](../zh/telemetry.md) · [目次](README.md)

<a id="opentelemetry"></a>
## OpenTelemetry

Cruise はワークフローのトレース、実行メトリクス、上限付きの失敗コマンド stderr ログを OTLP HTTP/protobuf でエクスポートできます。テレメトリーはデフォルトで無効であり、ワークフロー YAML の変更は不要です。

```sh
CRUISE_OBSERVABILITY_ENABLED=true \
OTEL_SERVICE_NAME=cruise \
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf \
cruise exec --config cruise.yaml
```

- `CRUISE_OBSERVABILITY_ENABLED=true`（大文字・小文字を区別しない）で有効になります。`OTEL_SDK_DISABLED=true` が指定されると、そちらが優先されます。
- エンドポイントの上書きがなければ、有効なテレメトリーは `http://localhost:4318` を使います。ベースエンドポイントには `/v1/traces`、`/v1/metrics`、`/v1/logs` が追加されます。`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`、`OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`、`OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` を指定すると、シグナルのパスを含めてそれぞれの URL がそのまま上書きされます。
- `OTEL_EXPORTER_OTLP_HEADERS` では、認証ヘッダーをカンマ区切りの URL エンコード済み `key=value` で指定します。ワークフロー YAML ではなく、環境変数またはシークレットマネージャーを使ってください。`OTEL_EXPORTER_OTLP_TIMEOUT` はリクエストタイムアウトをミリ秒単位で設定します。シグナル固有の `OTEL_EXPORTER_OTLP_TRACES_*` / `OTEL_EXPORTER_OTLP_METRICS_*` / `OTEL_EXPORTER_OTLP_LOGS_*` の値が優先されます。
- サポートされるのは HTTP/protobuf のみです。gRPC と HTTP/JSON のプロトコル指定は HTTP/protobuf にフォールバックします。圧縮は任意で、`OTEL_EXPORTER_OTLP_COMPRESSION=gzip` を指定できます。
- リソースのデフォルトは `service.name=cruise` とパッケージの `service.version` です。`OTEL_RESOURCE_ATTRIBUTES` でリソース属性を追加または上書きできます。サービス名には `OTEL_SERVICE_NAME` が優先されます。

メトリクスは `cruise.operation.count` と `cruise.operation.duration`（秒）で、ラベルは `cruise.operation` と `cruise.outcome` のみです。さらに `cruise.retry.count` があり、ラベルは `cruise.retry.action` と `cruise.retry.reason` のみです。セッション／ワークフロー／モデルの識別子はトレース専用で、メトリクスラベルにはなりません。トークン使用量とコストは OpenTelemetry にエクスポートされません。これらは `cruise usage` に記載のとおりローカルに記録されます。

プロンプト／入力テキスト、エージェント出力、コマンド本文、作業パス、例外メッセージがスパンまたはメトリクスに自動的に付加されることはありません。セッション ID、ワークフローファイル名、ステップ名／モデル名、CI チェック／ジョブ名と参照、および明示的に指定されたリソース属性はエクスポートされます。これらの値に秘密情報を含めないでください。

完了した失敗シェル試行では、`cruise.command.stderr` という ERROR OTLP ログが追加で出力される場合があります。ログ本文では認証情報ヘッダー、既知のトークン形式、秘密鍵ブロック、URL の userinfo／クエリーなどが秘匿され、最大 8,192 バイトに切り詰められます。成功したコマンドの stderr はエクスポートされません。

秘匿処理はパターンに基づくものであり、任意の秘密情報や個人データが必ず除去される保証ではありません。キャプチャされた stderr に機密性のあるパス、メッセージ、業務データが残る可能性があります。実際のワークロードでテレメトリーを有効にする前に、バックエンドログへのアクセスを認可された運用担当者に限定し、短い保持期間を設定してください。Cruise はバックエンドの RBAC や保持ポリシーを設定しません。これらはデプロイ側の責任であり、ログを有効にしても変更されません。

通常の CLI 終了時（ワークフローエラーを含む）にはプロバイダーがフラッシュされます。コレクター／エクスポーターの失敗によってワークフロー結果が変わることはありません。
