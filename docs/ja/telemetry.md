<a id="telemetry-guide"></a>
# テレメトリーガイド

**Languages:** [English](../en/telemetry.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/telemetry.md) · [简体中文](../zh/telemetry.md) · [目次](README.md)

<a id="opentelemetry"></a>
## OpenTelemetry

[TAKT のオブザーバビリティ対応](https://github.com/nrslib/takt/blob/main/docs/observability.md)に着想を得て、Cruise はワークフローのトレース、実行メトリクス、上限付きの失敗コマンド stderr ログを OTLP HTTP/protobuf でエクスポートできます。テレメトリーはデフォルトで無効であり、ワークフロー YAML の変更は不要です。

```sh
CRUISE_OBSERVABILITY_ENABLED=true \
OTEL_SERVICE_NAME=cruise \
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf \
cruise exec --config cruise.yaml
```

- `CRUISE_OBSERVABILITY_ENABLED=true`（大文字・小文字を区別しない）で有効になります。`OTEL_SDK_DISABLED=true` が指定されると、そちらが優先されます。テレメトリーが無効の場合、エクスポーターもネットワーククライアントも作成されません。
- エンドポイントの上書きがなければ、有効なテレメトリーは `http://localhost:4318` を使います。ベースエンドポイントには `/v1/traces`、`/v1/metrics`、`/v1/logs` が追加されます。`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`、`OTEL_EXPORTER_OTLP_METRICS_ENDPOINT`、`OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` を指定すると、シグナルのパスを含めてそれぞれの URL がそのまま上書きされます。
- `OTEL_EXPORTER_OTLP_HEADERS` では、認証ヘッダーをカンマ区切りの URL エンコード済み `key=value` で指定します。ワークフロー YAML ではなく、環境変数またはシークレットマネージャーを使ってください。`OTEL_EXPORTER_OTLP_TIMEOUT` はリクエストタイムアウトをミリ秒単位で設定します（SDK のデフォルト: 10,000）。シグナル固有の `OTEL_EXPORTER_OTLP_TRACES_*` / `OTEL_EXPORTER_OTLP_METRICS_*` / `OTEL_EXPORTER_OTLP_LOGS_*` の値が優先されます。
- サポートされるのは HTTP/protobuf のみです。gRPC と HTTP/JSON のプロトコル指定は HTTP/protobuf にフォールバックします。圧縮は任意で、`OTEL_EXPORTER_OTLP_COMPRESSION=gzip` を指定できます。
- リソースのデフォルトは `service.name=cruise` とパッケージの `service.version` です。`OTEL_RESOURCE_ATTRIBUTES` でリソース属性を追加または上書きできます。サービス名には `OTEL_SERVICE_NAME` が優先されます。

セッション ID の割り当て後、`cruise.execution` スパンはリポジトリのクローン作成（セッションの永続化より前）を含む準備から終了処理までを記録します。CLI の plan/exec/run、バックグラウンドのプランニングワーカー、TUI/WebUI が共有する Application API が対象です。並列セッションにはそれぞれ独立した実行ルートがあります。ルート属性には `cruise.execution.kind` と `cruise.session.id` が含まれます。設定を利用できる場合は `cruise.workflow.kind` と `cruise.workflow.name` も設定されます。ワークフロー名には組み込み名または最終的なファイル名が使われ、完全なパスは使われません。準備に失敗すると、ID は割り当てられていてもセッションが永続化されていないトレースが残る場合があります。

| スパン | 範囲 |
|---|---|
| `cruise.execution` | セッション ID で相関付けされたセッション操作 |
| `cruise.phase` | 実際に行われた `prepare`、`workspace`、`planning`、`workflow`、`pull_request`、`finalize` の処理 |
| `cruise.workflow` | workflow フェーズ内のエンジン実行 |
| `cruise.step` | 1 つのステップ。並列の子ステップは並列ステップの下に兄弟として記録されます |
| `cruise.agent` | 再試行のライフサイクルを含むプロンプトのバックエンド呼び出し |
| `cruise.attempt` | バックエンドまたはシェルの 1 回の試行。1 始まりの `cruise.retry.attempt` と選択されたバックエンド／モデルを含みます |

フェーズは、その処理が実行された場合にのみ出力されます。スキップされたプランニングや、無効または不要な PR 作成のために、成功を表すだけのプレースホルダーフェーズが出力されることはありません。`cruise.retry` スパンイベントには、バックオフ／フォールバックの処理、固定された再試行理由、試行回数、モデル切り替えが記録されます。SDK フォールバックと、それとは独立したコマンド／プロンプトのレート制限再試行ループも、再試行ポリシーを変更せずに計測されます。

GitHub のネイティブ squash merge がマージ可能性未確認のため拒否された場合、テレメトリーが無効でも、観測された `mergeable` と `mergeStateStatus` の値が保存済みセッションの失敗情報に含まれます。有効な場合は、テレメトリーにも同じ値が `squash_merge` ステップのスパン属性 `cruise.github.mergeable` と `cruise.github.merge_state_status` として記録されます。

同じスパンには、最終的な GitHub の準備状態の観測も含まれます。

| 属性 | 値 |
|---|---|
| `cruise.github.waiting_for` | `CI registration`、`current-head CI registration`、`current-head CI checks`、`GitHub mergeability` のいずれか。保留理由が観測されなかったか、準備状態が解決済みの場合は `none` |
| `cruise.github.ci_state` | 観測された `statusCheckRollup.state`。スナップショット取得前は `unobserved`、値がない場合は `unregistered` |
| `cruise.github.ci_head_matches` | 観測された CI ロールアップのコミットと想定 HEAD を比較する真偽値。CI コミットがない、またはまだ観測されていない場合も `false` |

最新の観測値はローカルに保持され、マージ操作の戻り時、またはその future が破棄されたときに 1 度だけ書き込まれます。外側のステップタイムアウトやキャンセルの場合も同様です。これによりポーリング属性の重複で SDK のスパン属性上限を使い切ることを防ぎます。準備完了またはマージ済みの結果では以前の待機理由がクリアされます。初期値の `none` / `unobserved` / `false` は CI 合格の証明にはなりません。コミット ID はエクスポートされません。

GitHub の squash merge コマンドが正常に完了せず、PR が想定 HEAD のまま Open であると確認できた場合、保存される失敗情報には固定された拒否理由と、取得可能な HTTP ステータスおよびコマンド終了コードが含まれます。同じスパンには `cruise.github.merge_refusal_reason` と、利用できる場合は数値の `cruise.github.merge_http_status` および `cruise.github.merge_exit_code` が記録されます。理由は `merge_not_performed`、`head_changed`、`base_changed`、`squash_disabled`、`merge_in_progress`、`unclassified` のいずれかです。最後の値は、応答が認識済みの標準メッセージと一致しなかったことを意味し、ステータスから原因を推測したものではありません。プロバイダーの生のメッセージや応答ヘッダーは保存もエクスポートもされません。これらの属性はメトリクスのラベルではありません。

並列の子ステップを含むスキップ済みステップは、ステップスパンも操作の観測値も出力しません。ステップの結果には `if.no-file-changes` などの事後条件の失敗も含まれます。その後の復旧で成功しても、失敗したステップが消去されることはありません。承認メニューのキャンセルは、CLI の終了コードを成功のままにしてキャンセルとして記録されます。プランニングまたはタイトル生成中の割り込みは、処理を排出した後もエラーを返します。Execute now を選ぶと、プランニングが完了してから別の実行ルートが開始します。

結果には `success`、`failed`、`error`、`cancelled`、`timeout`、`paused` があります。成功以外の操作にはエラーステータスと、`command_exit`、`rate_limit`、`timeout`、`cancelled` などのペイロードを含まない `error.type` が記録されます。スパンのステータス説明は `error.type` と同じです。既存のバックエンド再試行分類は、ネットワーク、プロバイダー、モデルの各失敗を区別します。復旧した再試行では失敗した試行が記録に残りますが、最終的に成功した実行を失敗として扱うことはありません。Esc キーで一時停止した exec は再開可能な状態のまま CLI を成功で終了しますが、そのルートの結果は `paused` です。

失敗したシェル試行には、数値の `cruise.command.exit_code`、または Unix でシグナルにより終了した場合は `cruise.command.signal` が含まれます。最終的に失敗した結果には、そのステップスパンにも属性が付与されます。復旧したレート制限試行は、成功したステップに属性を付与しません。

完了したものの失敗したシェル結果では、これらのスパンに `cruise.command.stderr_present`（空白のみを含め、キャプチャされた stderr が空でないかどうか）と `cruise.ci.diagnostic_status` も記録されます。

| 診断状態 | 意味 |
|---|---|
| `valid` | 上限と許可リストの制約を満たす `CRUISE_CI_DIAGNOSTIC_V2` または `CRUISE_CI_DIAGNOSTIC_V4` マーカーが受理された |
| `invalid` | V2 または V4 のマーカー候補は存在したが、検証を通過したものがなかった |
| `unsupported` | 未サポートの数値マーカーバージョンのみが認識された |
| `missing` | キャプチャされた stderr に CI マーカー候補が認識されなかった |

後続の行が不正または未サポートでも、最後に現れた有効な V2 または V4 マーカーが採用されます。有効なマーカーがない場合は `invalid` が `unsupported` より優先されます。候補はマーカートークンで始まります。検出に限り、前に ASCII 空白があっても認識します。両バージョンとも上限は 16,384 バイトで、許可リストにあるスカラー項目を含みます。V2 では後続フィールドを認めません。V4 では厳密に検証された JSON 詳細ペイロードが必要です。未知のバージョン、未知の JSON フィールド、不正な値はエクスポートされません。不正な任意参照は省略されますが、有効なチェック名／状態は保持され、詳細は不完全として示されます。`missing` は診断マーカーのない無関係なコマンドや古いワークフローを表すこともあります。これらの状態が示すのはキャプチャされた出力であり、根本の CI 失敗ではありません。完了したシェル結果を伴わない spawn エラー、タイムアウト、キャンセルでは、これらのフィールドは追加されません。2 つの属性はいずれもトレース専用で、メトリクスラベルではありません。

Jcode の失敗では、影響を受けた `cruise.attempt` スパンに固定診断が追加されます。

| 属性 | 意味 |
|---|---|
| `cruise.agent.failure_stage` | SDK の起動、API イベント処理、ターン終了処理など、失敗が検出された範囲が限定された段階 |
| `cruise.agent.failure_reason` | 型付き SDK/API/ターン失敗に対する、固定された許可リスト内の理由。不明なケースは `unknown` のままとし、HTTP ステータス、認証、プロバイダー内部の原因を推測しません |

これらの属性はトレース専用で、メトリクスラベルではありません。生の SDK エラーメッセージ、停止メッセージ、プロンプト、出力はエクスポートされません。

組み込みの `follow-up/wait-ci` コマンドは、上限付きの許可リスト診断も報告します。

| 属性 | 意味 |
|---|---|
| `cruise.ci.provider` | `github` または `gitlab` |
| `cruise.ci.failure_reason` | 固定された原因: `local_head_error`、`repository_query_error`、`repository_invalid`、`api_error`、`graphql_error`、`pr_head_missing`、`head_mismatch`、`rollup_head_missing`、`malformed_rollup`、`invalid_ci_state`、`invalid_check_count`、`ci_failed`、`unknown_state`、`clock_error`、`timeout` |
| `cruise.ci.state` | 最後に観測された許可リスト内の CI 状態、登録状態、または `unknown` / `unobserved`。任意のプロバイダーテキストは含まれません |
| `cruise.ci.head_matches` | 観測された GitHub PR HEAD がローカル HEAD と一致するかどうか。未観測の場合は省略されます。PR HEAD の一致はロールアップが最新である証明にはなりません |
| `cruise.ci.check_count` | 利用できる場合に観測された数値のチェック件数 |
| `cruise.ci.http_status` | 観測時の 100〜599 の数値 HTTP ステータス。不明な場合は省略されます |
| `cruise.ci.failed_checks` | `ci_failed` の場合に、失敗したチェック／ジョブの記述子（`name`、`state`、利用できる `run_id`、`job_id`、`url` 参照を含む）のコンパクトな JSON 配列 |
| `cruise.ci.failed_checks_truncated` | `ci_failed` の場合に、詳細が不完全であるか、不正な参照／項目が省略されたか、名前／参照が正規化または短縮されたかどうか |

失敗の詳細には最大 10 件のチェックを含め、名前は制御文字の除去と空白のトリミング後で最大 128 Unicode 文字に制限されます。GitHub は失敗したロールアップに対してのみ詳細応答を取得・ページングし、完了後に失敗した CheckRun の結論と失敗した StatusContext の状態を収集します。CheckRun 参照には、利用できる場合は workflow run ID が含まれます。Actions の job ID は、詳細 URL のリポジトリと run ID が観測されたリポジトリ／workflow run と一致する場合にのみ抽出されます。StatusContext 項目に run ID や job ID を推測して付与することはありません。GitLab では、pipeline ID、失敗した job ID、URL、ローカルトレースの取得に使う前に、詳細応答が観測された失敗 pipeline と一致することを確認します。参照 ID は最大 20 桁の正の 10 進数文字列です。URL は HTTP(S) に限り、最大 1,024 バイトで、userinfo、クエリー文字列、フラグメントを除去します。詳細がないか無効な場合も `ci_failed` は保持され、切り詰めフラグが設定されます。完全なマーカーを上限内に収める必要があれば、生成側は任意の参照を削除します。これらの記述子はトレース専用で、メトリクスラベルにはなりません。CI ジョブの完全なログはプロバイダー側に残り、テレメトリーにはログのコピーではなく参照が記録されます。

組み込みの `follow-up/sync-base` コマンドは、`cruise.sync_base.failure_reason` に `base_ref_query_error`、`base_ref_invalid`、`fetch_failed`、`merge_conflict`、`merge_failed` のいずれかをエクスポートします。CI と sync-base のスパン診断は、厳密に制限された stderr マーカーから取得されます。不正なマーカーフィールドやプロバイダーの生の応答はスパンに追加されません。無効または未サポートのものも含め、CI/sync-base のすべての機械処理用マーカー行は stderr ログから除外されます。保存済みワークフロースナップショットと以前に eject されたワークフローには、古い CI コマンドが残ります。それらのコマンドを置き換えずに V2 診断を読み取れます。`ci_failed` でチェック記述子を利用できない場合は、空の配列がエクスポートされ、`cruise.ci.failed_checks_truncated=true` が設定されます。V3 は未サポートのままです。V4 のチェック詳細を使うには、新しいスナップショットを作成するか、eject 済みワークフローを更新してください。すでにエクスポートされたスパンは変更されません。

`loop_protection` に分類されたエラーは、結果ベースの各包含スパンに `cruise.loop.from`、`cruise.loop.to`、`cruise.loop.max_retries`、そのエッジで受理された予算付き遷移数（拒否された遷移の直前の値）を示す `cruise.loop.traversals`、および `cruise.loop.skipped_step_count` を記録します。ステップ ID には既存のステップ名のプライバシーポリシーが適用されます。エッジの完全な一覧、スキップされたステップ一覧、例外メッセージはエクスポートされません。再試行予算とワークフローの動作は変わりません。

メトリクスは `cruise.operation.count` と `cruise.operation.duration`（秒）で、ラベルは `cruise.operation` と `cruise.outcome` のみです。さらに `cruise.retry.count` があり、ラベルは `cruise.retry.action` と `cruise.retry.reason` のみです。セッション／ワークフロー／モデルの識別子はトレース専用で、メトリクスラベルにはなりません。トークン使用量とコストは OpenTelemetry にエクスポートされません。これらは `cruise usage` に記載のとおりローカルに記録されます。

プロンプト／入力テキスト、エージェント出力、コマンド本文、作業パス、例外メッセージがスパンまたはメトリクスに自動的に付加されることはありません。セッション ID、ワークフローファイル名、ステップ名／モデル名、CI チェック／ジョブ名と参照、および明示的に指定されたリソース属性はエクスポートされます。これらの値に秘密情報を含めないでください。

完了した失敗シェル試行ごとに、その試行のネイティブな `trace_id` と `span_id` に関連付けられた `cruise.command.stderr` という名前の ERROR OTLP ログを追加で 1 件出力する場合があります。最終ステップで同じログが重複することはありません。成功したコマンドの stderr はエクスポートされず、サニタイズ後の出力が空または空白のみの場合はレコードが出力されません。spawn エラー、タイムアウト、完了したシェル結果を伴わないキャンセルでも stderr ログは出力されません。

ログ本文では、認識された認証情報ヘッダーと代入、認証スキーム、既知のトークン形式、秘密鍵ブロック、URL の userinfo／クエリー／フラグメント値を秘匿します。ANSI／制御シーケンスは照合の前に正規化されます。秘匿処理の後、UTF-8 の境界を守った先頭／末尾の抜粋が最大 8,192 バイトで出力されます。`cruise.command.stderr_truncated` が示すのは、サニタイズ後の出力がこの上限を超えたかどうかであり、認証情報が秘匿されたかどうかではありません。

秘匿処理はパターンに基づくものであり、任意の秘密情報や個人データが必ず除去される保証ではありません。キャプチャされた stderr に機密性のあるパス、メッセージ、業務データが残る可能性があります。実際のワークロードでテレメトリーを有効にする前に、バックエンドログへのアクセスを認可された運用担当者に限定し、短い保持期間を設定してください。Cruise はバックエンドの RBAC や保持ポリシーを設定しません。これらはデプロイ側の責任であり、ログを有効にしても変更されません。

通常の CLI 終了時（ワークフローエラーを含む）にはプロバイダーがフラッシュされます。コレクター／エクスポーターの失敗によってワークフロー結果が変わることはありませんが、シャットダウン時にエクスポートのタイムアウトまで待つことがあります。エクスポーター初期化設定が無効な場合は、秘匿処理済みの診断を表示して起動を停止します。強制終了時にフラッシュが保証されることはありません。

バックエンドがデータを受理しても、永続化が保証されるわけではありません。ローカル E2E 調査では、SDK の各エクスポートが別々に POST された場合、GreptimeDB はすべてのヒストグラム観測値を保持しました。一方、複数の `ResourceMetrics` を含む 1 つの OTLP リクエストにまとめて POST した場合は、先行するヒストグラムのポイントが失われました。SDK のペイロードにはすべての観測値が含まれ、ゲートウェイは HTTP 200 を返していました。この下流の取り込み問題に対処するための重複エクスポートは行いません。
