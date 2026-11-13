<a id="sessions-and-workspaces"></a>
# セッションとワークスペース

**Languages:** [English](../en/sessions.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/sessions.md) · [简体中文](../zh/sessions.md) · [目次](README.md)

<a id="session-management"></a>
## セッション管理

Cruise はセッションデータを `$XDG_DATA_HOME/cruise/sessions/`（デフォルト: `~/.local/share/cruise/sessions/`）に保存します。各 `state.json` には、タグ付きの `config` 参照が 1 つ含まれます。

```json
{"config":{"kind":"file","path":"/absolute/path/cruise.yaml"}}
```

その他にサポートされる種類は、`builtin_snapshot`（組み込みカタログ項目の任意の `name` を指定できます。例: `{"kind":"builtin_snapshot","name":"simple"}`。`name` のない古いセッションも引き続き読み込めます）、`repo_snapshot`（クローンからの相対 `relative_path` を指定）、および `inline_snapshot` です。`file` 参照はライブ参照で、ファイルへの変更は次回の読み込み時または実行中の再読み込み時に反映されます。`--skip-planning`（入力をプランとして使用）で作成したセッションでは、`"input": ""` と `"input_as_plan": true` が保存され、タスク本文は `plan.md` に格納されます。

スナップショット参照が読み込むのは、そのセッションが所有する `sessions/<session-id>/config.yaml` のみです。別の設定を再検出したり、暗黙にフォールバックしたりすることはありません。組み込み、インライン、一時クローンのワークフローは、解決済み YAML としてここにシリアライズされるため、元のソースやクローンがなくなった後も実行できます。`config_source` と `config_path` を含む古い `state.json` ファイルは移行も読み込みもされません。この形式を使う場合は、新しいセッションを作成してください。

<a id="runtime-file-layout"></a>
### ランタイムファイルの配置

Cruise は [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/latest/) に従い、ランタイムファイルを 3 つのディレクトリに分けて保存します。

| 種類 | パス |
|------|------|
| ユーザーのワークフロー YAML 設定（`workflows/*.yaml` / `*.yml`） | `$XDG_CONFIG_HOME/cruise/workflows/`（デフォルト: `~/.config/cruise/workflows/`） |
| ユーザーのワークフローデフォルト | `$XDG_CONFIG_HOME/cruise/default.yaml`（デフォルト: `~/.config/cruise/default.yaml`） |
| プロジェクトのワークフローデフォルト | `./.cruise/default.yaml` |
| アプリケーション設定（`config.json` または `config.yaml`） | `$XDG_CONFIG_HOME/cruise/`（デフォルト: `~/.config/cruise/`） |
| セッション、ワークツリー、一時的な `--repo` クローン | `$XDG_DATA_HOME/cruise/`（デフォルト: `~/.local/share/cruise/`） |
| 状態ファイル（`history.json`、`new_session_draft.json`、インストールごとの `installations/<installation-id>/update-transaction.json`） | `$XDG_STATE_HOME/cruise/`（デフォルト: `~/.local/state/cruise/`） |
| WebUI インスタンスレジストリ、ロック、デーモンログ | `$XDG_STATE_HOME/cruise/webui/`（`instance.json`、`instance.lock`、`webui.log`、および任意の `webui.log.1`） |
| 使用状況レコード | `$XDG_STATE_HOME/cruise/usage.ndjson`（デフォルト: `~/.local/state/cruise/usage.ndjson`） |

Windows では、`XDG_*` 変数が未設定の場合、デフォルトは `%APPDATA%\cruise`（設定）、`%LOCALAPPDATA%\cruise\data`（データ）、`%LOCALAPPDATA%\cruise\state`（状態）です。これらの変数が設定されていない場合は上記の `~/` のデフォルトが適用され、`~\` は `~/` と同様に展開されます。

> **`~/.cruise/` から移行する場合:** 以前のバージョンではすべてのファイルを `~/.cruise/` に保存していました。`*.yaml` / `*.yml` は `~/.config/cruise/workflows/` に、`config.json` は `~/.config/cruise/` に、`sessions/` と `worktrees/` は `~/.local/share/cruise/` に、`history.json` と `new_session_draft.json` は `~/.local/state/cruise/` に移動してください。ワークツリーディレクトリを移動する場合は `git worktree move`（または `git worktree repair`）を使用します。
>
> **ワークフロー設定が以前 `~/.config/cruise/*.yaml` / `*.yml` にあった場合:** ユーザー設定の自動検出でワークフロー YAML を探す場所は、現在 `workflows/` サブディレクトリのみです。ファイルを `~/.config/cruise/workflows/` に移動してください。`~/.config/cruise/` 直下に古いワークフロー YAML ファイルが残っていると、cruise は警告を表示します。アプリケーション設定 `config.yaml` とワークフローデフォルト `default.yaml` はそのディレクトリに残しても警告の対象になりません。

<a id="session-lifecycle"></a>
### セッションのライフサイクル

1. **`cruise plan "task"`** -- 組み込みの plan ステップを分離された計画用ワークツリーで実行し、実装計画を生成します。その後、stdin が対話型 TTY の場合はプラン承認メニューを表示し、非 TTY の場合はプランを自動承認します。
2. **`cruise --plan "task"`** -- セッションを直ちに作成し、バックグラウンドでプランを生成します。後から `cruise list` で確認できます。
3. **`cruise draft "task"`** -- plan ステップを実行せず、タスクを `Draft` セッションとして記録します。準備ができたら `cruise list` の **Generate Plan** からプランニングを開始します。
4. **プラン承認メニュー** -- 次のいずれかを選択します。
   - **Approve** -- セッションを実行可能な状態にします。
   - **Fix** -- フィードバックを入力し、その内容を使って plan ステップを再実行します。
   - **Ask** -- 質問します。回答は記録されますが表示されず、その後メニューに戻ります。
   - **Verify** -- Quint と Alloy の仕様を生成し、それらのチェックを実行して、保存済みプランを変更せずにレポートとその解釈を表示します。プランを修正するには **Fix** を使います。
   - **Execute now** -- 承認を省略し、直ちに実行します。
   - **Publish as Issue** -- `plan.md` を GitHub Issue（または GitLab Issue）として公開し、ローカルセッションを削除します。

   TUI の **Fix Plan**、**Ask About Plan**、**Verify Spec** と、WebUI の **Fix**、**Ask**、**Verify** は、使用可能なプランがあれば **Awaiting Approval** または **Planned** のどちらでも利用できます。Verify はメニューアクションであり、Ask 内の `/verify` コマンドではありません。利用には PATH 上の `quint`（0.33.0 推奨）と Alloy 用の Java 17 以降が必要です。`CRUISE_ALLOY_JAR` が指定されていれば Alloy はその JAR を使い、指定されていなければ Cruise が Alloy 6.2.0 をデータディレクトリにダウンロードし、チェックサムを検証します。レポートとログはセッションが削除されるまでセッションの `formal/<run-id>/` ディレクトリに保持され、リポジトリには保存されません。Quint の各 parse、typecheck、simulation には 60 秒の制限があります。モデルチェックには `formal_verify.model_check_timeout_seconds`（デフォルト 900 秒）が適用されます。Alloy コマンドは逐次実行されるため、N 個のコマンドにはその制限時間の最大 N 倍に加え、セットアップやその他のチェックの時間がかかることがあります。制限付きチェックに合格しても、モデル化した範囲を超える証明にはなりません。

   **Approve** または **Execute now** を選ぶと、`-f/--flag` が指定されていない場合は、宣言済みワークフローフラグを先に選択します。次に **step skip selector** に、それらの値で有効になっているステップのみが表示されます（グループ化されたステップは親と子として表示されます）。候補がなければセレクターは表示されません。この実行でスキップするステップを切り替えて選択してください。表示された候補からの選択だけが `$XDG_STATE_HOME/cruise/history.json` に設定ファイルごとに保存され、同じ設定を使う次のセッションで事前選択されます。どちらかのセレクターをキャンセルすると、フラグの途中変更を保存したり、セッションを承認・実行したりせず、プラン承認メニューに戻ります。Execute now は承認時のフラグ値を明示的な割り当てとして引き継ぐため、実行開始時の再確認を抑止します。Approve 後の `cruise run` では、保存済みフラグと CLI の上書きを使い、フラグセレクターもスキップ選択の再確認も表示しません（[ワークフローフラグ](cli-reference.md#cruise-run)を参照）。

5. **`cruise run`** -- ID を省略すると、実行待ちのセッション（Planned、Running、Failed、Suspended）を選択し、該当するものが複数ある場合は選択を促します。`$XDG_DATA_HOME/cruise/worktrees/<session-id>/` にある Git ワークツリーを再利用（または作成）し、ワークフローを実行します。デフォルトでは、実行後にブランチコミットが残っている場合に PR を作成し、その後、設定された `after-pr` ステップを実行します。組み込みワークフローでは `-f after_pr` を指定すると、この PR 後処理を有効にします（マージは行いません）。`-f merge` を指定すると、同じ PR 後処理を 1 回実行した後、組み込みの同期的な squash merge を行います。成功した実行で新しいブランチコミットがない場合、PR 作成はエラーなしでスキップされます（既存のブランチコミットは引き続き対象です）。有効にした PR 後アクションも理由を示してスキップされます。ワークフローレベルで `create_pr: false` にすると、自動コミット、push、PR 作成、`after-pr`、PR ベースのクリーンアップがスキップされ、通常のセッション、成果物、ワークツリーは保持されます。有効にしたアクションは理由を示してスキップされ、merge フラグが no-PR 設定を上書きすることはありません。明示的に設定された `if.no-file-changes` の失敗・再試行動作は変わりません。公開に失敗した場合は、リポジトリセッションとローカルワークツリーセッションのどちらも **PR Failed** になります。ワークフローを再実行せずに公開を再試行するには **Create PR** を使います。

   GitHub の現在の HEAD で競合が確認された場合、CI の失敗が報告されない限り、`wait-ci` はコミットとロールアップを検証した後、速やかに終了します。CI が失敗した場合は、実行中の Actions スイートの完了待ちを含め、診断と修復の経路が維持されます。古い PR HEAD の場合は、引き続き反映待ちの猶予期間が適用されます。GitHub の組み込み `wait-ci` ステップはローカルの `HEAD` を固定します。準備完了の条件は、PR HEAD と CI ロールアップのコミットがその期待するコミットに一致し、`statusCheckRollup.state` が存在して `SUCCESS` であり、そのコミット上の `workflowRun` を持つ GitHub Actions チェックスイートがすべて `COMPLETED` であることです。完了したスイートの結論が失敗なら CI 失敗となります。サードパーティアプリなどの `workflowRun` を持たないスイートはブロック要因になりません。準備完了が連続 2 回のポーリングで確認される必要があり、準備未完了のポーリングが 1 回でもあれば連続回数はリセットされます。PR HEAD の不一致は API 反映のため 90 秒の猶予期間中は保留されます。その間に追いつけば待機は成功する可能性がありますが、猶予期間または全体の期限を過ぎても不一致が続く場合は、既存の `head_mismatch` 診断で失敗します。組み込みステップは 60 分のステップタイムアウト内で、最大 55 分（3300 秒）、5 秒間隔でポーリングします。未登録または保留中のチェックは成功とはみなされません。既存のスナップショットセッションは、明示的に更新されるまで保存済みの wait コマンドとタイムアウトを保持します。組み込みコマンドを変更しても、それらは書き換えられません。

   組み込みの GitHub squash merge にも同じ CI 準備完了ゲートが適用されます。PR HEAD と既存のロールアップコミットが想定されたローカル `HEAD` に一致し、ロールアップ状態が `SUCCESS` であり、そのコミット上の `workflowRun` を持つ GitHub Actions チェックスイートがすべて `COMPLETED` である必要があります。完了したスイートの結論が失敗なら CI 失敗となり、`workflowRun` を持たないスイートはブロック要因になりません。準備完了は連続 2 回のポーリングで確認される必要があり、準備未完了のポーリングがあると連続回数はリセットされます。PR HEAD の不一致は API 反映のため最大 90 秒保留されます。この間に追いつく一時的な不一致は成功する可能性がありますが、猶予期間または準備完了の全体期限を過ぎても続く場合は失敗します。組み込みの `wait-ci` は従来の `head_mismatch` 診断を保持します。また、このマージでは GitHub のマージ可能性が `UNKNOWN` から解決するのを待ち、最大 10 分間、2 秒間隔でポーリングします（より短いステップタイムアウトがある場合はそれが適用されます）。このランタイムチェックは、既存の保存済みワークフロースナップショットを書き換えずに保護します。競合、不正な応答、満たされていないポリシー要件またはレビュー要件があれば、引き続きマージを拒否します。最終マージ要求は一度だけ送られ、SHA に固定されます。

   - 直前の段落にある競合による拒否のうち、GitHub でベースブランチより遅れている、または分岐していることが確認された状態と `CONFLICTING` / `DIRTY` のマージ可能性は例外です。再試行できる準備状態はこれらに限られます。マージ時には GitHub の compare API で、PR に現在のベースが含まれているか確認します。組み込みの squash-merge ステップは該当ケースを `follow-up/sync-base` にルーティングし、その後、再試行前に `sync-base` → `resolve-conflict` → `push-after-sync` → `wait-ci` → `fix-ci-error` の順に進みます。`squash-merge` からその `if.fail` の対象へのネイティブな `ResyncRequired` ルーティングだけが、新しい after-PR パスを開始します。この場合、`squash-merge` → 対象へのエッジを除くすべての after-PR エッジで `budgeted_traversals` をリセットし、グループ再試行回数をクリアしつつ、エッジの遷移履歴は保持します。再同期エッジの上限は `max_retries` です。一般的な `if.fail` では予算は共有されます。最悪の場合、`max_retries` 回まで再同期し、各パスで最大 `max_retries - 1` 回の CI 修正を反復します。再試行予算が 0 の場合、再同期はできません。それ以外の準備未完了は引き続き失敗となります。この祖先関係の確認と復旧は GitLab の動作を変更しません。

   確定したマージ拒否では、PR が想定された HEAD のまま Open であることを確認した後、固定された理由と、取得可能な HTTP ステータスおよびコマンド終了コードのみが記録されます。マージの成功確認やその他の確認エラーがある場合は、拒否の診断よりもそちらが優先されます。

   `MERGEABLE/UNSTABLE` は、固定されたコミットの CI が保留中または未登録・未更新の場合に限り一時的な状態です。マージ準備完了とは決してみなされません。現在の HEAD の CI 失敗は直ちに拒否され、CI がグリーンでも `UNSTABLE` が残っていれば拒否されます。

   PR に関連付けられたセッションは `$XDG_DATA_HOME/cruise/sessions/` に残り、PR がクローズまたはマージされた後に `cruise clean` で削除できます。`create_pr: false` を設定した通常のセッションは、PR を待たずにセッション、成果物、ワークツリーを保持します。

   PR のない対象のローカルワークツリーセッションでは、セッションを保持したまま squash merge をステージするか、明示的にマージしてワークツリー、ブランチ、セッションをクリーンアップできます。

<a id="model-and-fallback-notices"></a>
### モデルとフォールバックの通知

ステータスまたは情報の表示先がある場合、各プロンプト実行では Cruise が選択したモデルをそこに報告します。通知はすべてセッションの `run.log` にも書き込まれます。

```text
Model: provider/model:free:xhigh
```

再試行ポリシーによってモデルが切り替わる場合、Cruise は切り替えと後続モデルを報告します。

```text
Warning: Fallback: provider/primary -> provider/fallback (503, attempt 1/5)
  Error: API Error: 503 service temporarily unavailable
Model: provider/fallback
```

括弧内の詳細は、元のモデルが選択対象から外れた理由です。失敗メッセージにプロバイダーが返した HTTP ステータス（`503`、`429`、`400`、`404` など）が記載されている場合はその値が使われ、それ以外の場合は失敗クラス（`rate limit`、`server error`、`client error`、`missing model`、`network error`、またはバックエンドが送信前に拒否した参照に対する `unusable model`）が使われます。

元のバックエンドエラーメッセージは、代替モデルで成功した場合も含め、埋め込み改行を保持したまま `Error:` に続けて表示されます。

プロバイダー、パス、effort の接尾辞を含む完全なモデル参照が保持されます。Claude と外部コマンドのバックエンドは Cruise のモデル指定なしでも実行でき、その場合は通知でバックエンドを示すことがあります。jcode プロンプトに有効な Cruise モデルがない場合は、選択を jcode に委ねず、セットアップエラーで停止します。そのため `Model: default model (jcode)` は出力されません。通知は、ステータス出力が有効な場合はフォアグラウンド CLI の stderr に、TUI では既存の情報ログに、さらにセッションの `run.log` に送られます。デタッチされた `cruise --plan` ワーカーと CLI の対話型 `cruise run --all` ダッシュボードでは端末通知が抑制されるため、通知はセッションの `run.log` で確認してください。通知が stdout やプラン／Ask の回答内容に混在することはありません。既存のセッションライフサイクルとログ保持の規則は変わりません。特に、一時的な `cruise exec` セッションは完了後も引き続き削除されます。

> **`cruise exec`** は一時的なライフサイクルを持つ別経路です。プランニング、ワークツリー作成、PR 作成を行わずにカレントディレクトリで実行し、終端状態に達するとセッションを削除します。一時停止または中断された exec セッションは ID を指定すれば再開できます。`force_exec: true` を設定すると、直接の plan エントリーポイントでも同じ経路が有効になります。1 回だけ無効にするには `--no-force-exec` を使います。[`cruise exec`](cli-reference.md#cruise-exec)を参照してください。

<a id="cruise-list-actions"></a>
### `cruise list` のアクション

対話型セッション一覧には、セッションのフェーズに応じたアクションメニューが表示されます。

| フェーズ | 利用可能なアクション |
|-------|-------------------|
| **Draft** | Generate Plan、Delete、Back |
| **AwaitingInput** | Generate Plan、Delete、Back |
| **AwaitingApproval** | Approve、Publish as Issue、Edit Settings、Delete、Back |
| **Planned** | Run、Publish as Issue、Edit Settings、Replan、Delete、Back |
| **Running** | Resume、Tell*、Reset to Planned、Delete、Back |
| **Suspended** | Resume、Edit Settings、Reset to Planned、Delete、Back |
| **Failed** | Run、Edit Settings、Reset to Planned、Delete、Back |
| **PR Failed** | Create PR、Open PR*、Reset to Planned、Delete、Back |
| **Completed** | Open PR*、Merge PR*、Stage local merge**、Merge locally & cleanup**、Reset to Planned、Delete、Back |

`cruise list` には Verify は表示されません。Ask が既存の一覧にないのと同様です。

\* Open PR と Merge PR は、セッションに PR URL がある場合にのみ表示されます。Tell は実行中セッションの runner が稼働している場合にのみ表示されます。

\** ローカルマージアクションを利用できるのは、PR がなく、対象ブランチが記録されている適格なローカルワークツリーセッションです。マージインテントまたはコミットが記録されると Stage は利用できません。対象ブランチが記録されていない古いセッションも対象外です。

ワークツリーのブランチがある場合は、どのフェーズでも **View Diff** も利用できます。ブランチがすでに削除されている場合、差分の表示はエラーになります。

バックグラウンドのプランニングがまだ実行中の場合、`cruise list` に `Planning` が表示されることがあります。また、永続的な `plan_error` が書き込まれると `Plan Failed` が表示されます。これらの表示状態は `AwaitingApproval` に基づきます。`Edit Settings`、`Delete`、`Back` は常に利用できますが、空でない `plan.md` があり `plan_error` がない場合に限り `Approve` と `Publish as Issue` も表示されます。

- **Generate Plan** -- `Draft` または `AwaitingInput` セッションのプランを生成します（`AwaitingApproval` に遷移します）。
- **Approve** -- プランを承認し、セッションを Planned フェーズに遷移させます。
- **Publish as Issue** -- 変更せずに `plan.md` を、解決済みリポジトリの GitHub Issue（または GitLab Issue）として公開し、その後ローカルセッションを削除します。`@cruise` GitHub Action が検出できるよう、続けて `@cruise run` コメントも投稿するかを尋ねます（デフォルトは `AwaitingApproval` ではオフ、`Planned` ではオン）。Issue の作成には成功したもののコメント投稿に失敗した場合は、再試行できるようセッションが保持されます（既存の Issue を再利用し、重複作成はしません）。手動でコメントを投稿することもできます（GitLab では `@cruise run` コメントの確認は省略されます）。
- **Run / Resume** -- セッションを実行または続行します。
  致命的な PR 後処理の失敗からは、メインワークフローを再実行したり PR を再公開したりせず、チェックポイントに記録されたステップから再開します。明示的な `--flag` 上書きがあれば、それを保存済みフラグに適用します。after-PR ステージマーカーのない古いセッションでは、従来どおり after-PR ステップを再開時に最初から実行します。
- **Tell** -- 実行中セッションの次のエージェントプロンプトに指示を送信します。TUI と WebUI でも実行中に Tell を使えます。
- **Create PR** -- **PR Failed** セッションの公開を、ワークフローを再実行せずに再試行します。Cruise は残りの変更をコミットして push し、そのブランチの既存 PR を再利用するか、新しく作成します。まず表示されたエラーと forge の認証を確認してください（GitHub の HTTPS 認証情報については `gh auth setup-git` で Git を設定できます）。公開の失敗またはキャンセル後も、ワークツリーと既存 PR URL があればそれを含めて、セッションは **PR Failed** のまま保持されます。致命的な after-PR エラーが発生すると **Failed** になります。成功するとセッションは完了し、PR が作成または再利用された場合にのみ、設定済み after-PR ステップが実行されます。コミットがなく公開がスキップされた場合は、after-PR ステップを実行せずに完了します。Reset to Planned はワークフローを最初から再実行し、Delete はセッションとそのワークスペースを削除します。
- **View Diff** -- セッションブランチの変更を確認してメニューに戻ります。比較とクリーンアップの制限については [`cruise diff`](cli-reference.md#cruise-diff)を参照してください。
- **Replan** -- フィードバックを入力してプランを再生成します。セッションは Planned フェーズのままです。
- **Open PR** -- `gh pr view --web` でセッションのプルリクエストをブラウザーで開きます。
- **Merge PR** -- セッションのプルリクエストを人が開始してマージします（TUI と WebUI でも利用できます）。最初に PR の状態、マージ可能性、レビュー判定、各 CI チェックが表示されます。次に `Squash`（デフォルト）、`Merge`、`Rebase` から選択して確認します。確認画面には選択した方法が表示されます。Cruise は `gh pr merge <url> --squash|--merge|--rebase` を実行し、`--auto`、`--admin`、`--delete-branch` は追加しません。その後 GitHub が PR を Closed または Merged と報告した場合、`cruise clean` と同様に、そのセッションとワークツリー（および `--repo` クローンがあればそれ）だけをクリーンアップします。たとえばマージキュー内にあるなど、PR がまだ Open ならセッションは保持され、後から `cruise clean` で削除できます。すでに Closed または Merged の PR を再度マージすることはありません。その場合は `cruise clean` を実行してください。これは人の確認を必要とする GitHub 専用経路です。組み込みの `-f merge` は別の同期 squash 経路です。チェックと制約については [after-pr](../../skills/cruise-config/references/after-pr.md)を参照してください。
- **Stage local merge** -- セッションを削除したりコミットしたりせずに、セッションブランチをベースチェックアウトのインデックスに squash します。ワークツリーの未コミット変更は、先にセッションブランチへコミットされます。
- **Merge locally & cleanup** -- 1 つのベースコミットに squash した後、ワークツリー、ブランチ、セッションを削除します。クリーンアップに失敗した場合、squash をやり直さずに再試行できるようセッションを保持します。最初の squash では、記録された対象ブランチがローカルでチェックアウトされていること、追跡対象ファイルに変更のないベースであること、upstream より遅れていないことが必要です。Cruise は fetch も pull もしません。クリーンアップの再試行では、記録されたソースコミットが利用可能であること、ブランチが変更されていないこと、保持されているワークツリーが想定されたチェックアウトであり、追跡対象・未追跡ファイルに変更がないことを確認します。競合は報告され、ロールバックが試みられます。ロールバックに失敗した場合も報告され、AI が競合を解決することはありません。これらのアクションは `--repo` セッションと current-branch セッションには適用されず、`cleanup_after_pr` や `merge` フラグにも依存しません。
- **Reset to Planned** -- 現在のステップをクリアしてセッションを Planned フェーズに戻し、最初から再実行できるようにします。
- **Delete** -- セッションを完全に削除します。
- **Back** -- セッション一覧に戻ります。

<a id="workspace-mode"></a>
## ワークスペースモード

`cruise plan` または `cruise "task"` から Execute now を選ぶと、`-f/--flag` を指定していない限り、承認メニューでスキップするステップを選ぶ前にフラグを選択します。その値は実行時に再確認せず引き継がれます。その後の `cruise run [id]`（`cruise list` の Run を含む）では、保存済みフラグと CLI の上書きを使い、フラグセレクターは表示しません。たとえば `cruise plan -f merge "task"` の後に Approve すると、`cruise run <id>` は確認なしで保存済みの `merge` 値を使います。これとは別に、`cruise exec` と `force_exec` のエントリーポイントでは、フラグが宣言され、`-f/--flag` が指定されていない場合、TTY の stdin で[ワークフラグセレクター](cli-reference.md#cruise-run)が表示されます。dry-run と非 TTY 実行では確認されません。

`cruise run` が新しいセッションを開始すると、ワークスペースモードの選択を促します。

```
? Where should cruise execute?
> Create worktree (new branch)
  Use current branch
```

| モード | 説明 |
|------|-------------|
| **Worktree**（デフォルト） | `$XDG_DATA_HOME/cruise/worktrees/<session-id>/`（デフォルト: `~/.local/share/cruise/worktrees/<session-id>/`）に分離された Git ワークツリーを作成します。後述のとおり、HEAD または設定された `base_branch` を起点に、新しいブランチ `cruise/<session-id>-<sanitized-input>` をチェックアウトします。PR 作成には `gh` CLI（GitLab リポジトリでは `glab`）が必要です。 |
| **Current branch** | アクティブブランチ上の現在のリポジトリで直接実行します。ワークツリーも PR も自動作成されず、自動マージも行われません。`after_pr` または `merge` が有効になっている場合、利用できない PR 後アクションは理由を示してスキップされます。 |

`--all` では常に worktree モードが使われます。単独の `cruise run` はセッションにすでに記録されたワークスペースモードを維持し、非対話環境（stdin がパイプ）の新規セッションでは worktree モードがデフォルトになります。`--repo`（または WebUI のリポジトリピッカー）で作成されたセッションは常に Worktree モードに固定されます。一時クローンから変更を外部へ反映する唯一の方法が PR であるため、モード選択は省略され、current-branch モードは利用できません。

<a id="current-branch-mode-constraints"></a>
### Current-branch モードの制約

- 新規の `cruise run` current-branch セッションでは、作業ツリーがクリーンである必要があります（追跡対象ファイルに未コミット変更がないこと。未追跡ファイルは無視されます）。`cruise exec` と `force_exec` のセッションは、警告を表示したうえで変更のある状態から開始できます。Cruise は既存の変更を stash、commit、reset しません。
- ブランチがチェックアウトされている必要があります（detached HEAD は不可）。
- 再開時には、アクティブブランチがセッション開始時に記録されたブランチと一致している必要があります。

<a id="worktree-isolation"></a>
### ワークツリーの分離

- PR に関連付けられたセッションでは、PR がクローズまたはマージされるまでワークツリーが保持されます。削除するには `cruise clean` を実行してください。PR のない対象セッションでは、代わりにローカルマージとクリーンアップを実行できます。
- PR 作成後すぐにローカルワークツリーとブランチを自動削除するには、設定で `cleanup_after_pr: true` を指定するか、実行時に `--cleanup-after-pr` を渡します。設定を上書きして保持するには `--no-cleanup-after-pr` を使います。
- 明示的な **Merge locally & cleanup** も、`cleanup_after_pr` とは独立して、squash コミット後に対象のローカルワークツリーとブランチを削除します。

新しいワークツリーは、ユーザーまたはプロジェクトの `default.yaml` に `base_branch` が設定されていない限り、リポジトリの現在の HEAD から作成されます。ベースが設定されている場合、Cruise はまずローカルブランチを探し、次に `origin/<base>` を探します。`auto_fetch: true` の場合は `origin` からそのブランチだけを fetch し、更新されたリモート追跡 ref を優先します。fetch に失敗すると警告を表示してローカル ref にフォールバックし、ベースが見つからなければエラーになります。設定したベースはセッションとともに保存され、PR または MR の対象として使用されます。設定がなければ forge CLI がデフォルトを選びます。既存のワークツリーは再利用され、再 fetch やベースの再解決は行われません。

<a id="copying-files-into-the-worktree"></a>
### ファイルをワークツリーにコピーする

ワークフロー開始前にファイルまたはディレクトリを新しいワークツリーへコピーするには、リポジトリのルートに `.worktreeinclude` ファイルを作成します。

```
# .worktreeinclude
.env
.cruise/
secrets/config.yaml
```

各行に相対パス（ファイルまたはディレクトリ）を指定します。安全のため、絶対パスと `..` による上位ディレクトリへの移動は無視されます。

<a id="stale-session-detection"></a>
## 古いセッションの検出

`cruise list`、TUI（`cruise`）、または WebUI のセッション詳細画面がセッションを読み込む際、Running フェーズのセッションについて稼働状態を確認します。PID と開始時刻で特定された runner プロセスがすでに稼働していなければ、セッションは自動的に Suspended フェーズへ遷移します。これにより、クラッシュや強制終了の後にセッションが Running のまま永久に残ることを防ぎます。

Suspended セッションは `cruise list` または TUI から再開するか、Planned にリセットできます。`run --all` コマンドも Planned セッションとともに Suspended セッションを実行対象にします。

実行には固定サイズの有向 **Graph** を使い、コンパイル済みステップごとに安定したノードが 1 つあります（並列ブロックも 1 単位として扱われます）。`max_retries` を増やしてもグラフは拡大しません。バージョン付きの `dag.json` チェックポイントが、次のステップ、受理されたエッジ遷移、予算付き遷移、グループ呼び出しの再試行回数、`{prev.*}` 変数、ファイルスナップショットの正本です。チェックポイントは最初のステップの前、および受理された遷移、通常完了、ループ保護の後にアトミックに書き込まれます。永続化に失敗すると、次の副作用が起きる前に実行を停止します。ステップの途中で中断した場合はステップ開始前のチェックポイントが保持されるため、外部コマンドの副作用が exactly-once になる保証はありません。

通常の再開とプロンプト／モデルの再読み込みではカウンターが保持されます。`--max-retries` は、値を下げた場合も含め、保存済みカウントに新しい上限を適用します。拒否された遷移は保留状態のままになり、再開しても予算は補充されません。再開位置を変更してもカウンターは保持されます。明示的な再起動だけが新しい実行を開始し、以前のチェックポイントをアーカイブします。ホットリロードで現在のステップが削除された場合、ワークフローは以前のものを保持し、黙って先頭に戻ることはありません。

メインワークフローでは互換性のためのファイル名 `dag.json` を使い、after-PR では独立した `after-pr-dag.json` チェックポイントとカウンターを使います。チェックポイント済みの after-PR 失敗は、メインワークフローを再実行したり再公開したりせずに再開できます。IPC 名 `get_session_dag` は引き続き使われます。バージョンのない従来のグラフは表示できますが、推測したカウンターで再開することはできません。チェックポイントがない、破損している、または未知のバージョンである場合は、データを削除せずに再開を停止します。最初から実行するにはセッションを明示的に再起動してください。

<a id="parallel-session-execution"></a>
## セッションの並列実行

Run All は、失敗したステップから同じ worker 内で、対象となる失敗を自動的に再試行できます。WebUI の設定、TUI の Run All 画面で `e` キー、または `cruise config --set-auto-requeue-max-attempts <N>` で 0 以上の上限を設定します。デフォルトは `0` で、再試行は無効です。分類されていないコマンドエラー、対象となるプロバイダーエラー、変更なしエラーは再試行されることがありますが、キャンセル、設定、認証、認可エラーは対象外です。各試行では通常の失敗通知も表示されます。失敗したステップがある完了済みの実行を、失敗として再分類することはありません。

セッションごとの保存カウントはバッチをまたいで使われる生涯上限です。手動の Run または Retry ではリセットされませんが、Reset to Planned ではリセットされます。Failed セッションはバッチ開始時の対象にならず、単独セッションの実行と exec セッションも自動再試行されません。上限を下げてもカウントは消去されません。カウントはセッション詳細、CLI の list JSON、バッチ概要に表示されます。

WebUI、TUI、CLI は `run --all` で複数セッションを同時に実行できます。

- **WebUI/TUI**: 並列度は、選択されたアプリケーション設定ファイル（`$XDG_CONFIG_HOME/cruise/` 以下の `config.json` または `config.yaml`）にある `run_all_parallelism` で制御されます。`cruise config --set-parallelism <N>` で設定でき、デフォルトは `1` です。
- **CLI**: 1 回の実行だけに適用する上書きには `cruise run --all --parallelism <N>` を指定します（デフォルトは `1`、つまり逐次実行）。保存済みの WebUI/TUI 設定は読み取られず、変更もされません。対話型端末で `--dry-run` を指定していない場合、CLI はスケジュールされた各セッションのタイトル、現在のステップ、状態、経過時間を示すライブダッシュボードも表示します。詳細なエージェント出力は `sessions/{id}/run.log` に保持されます。非 TTY と dry-run では通常のログ出力と最後の概要が使われます。

バッチスケジューラーの動作:
- Planned と Suspended のセッションを初期候補にします。
- 最大 `N` セッションを同時に起動します。
- worker の空きスロットがある間、200 ms ごとに新たに追加された実行可能な Planned または Suspended セッションを再スキャンし、バッチの実行中に作成されたセッションも自動的に取り込みます。
- 完了順にかかわらず、スケジュール順に結果を返します。

<a id="new-session-form-persistence"></a>
## 新規セッションフォームの永続化

WebUI と TUI は、次の 2 種類の状態をセッションをまたいで保持します。

- **Draft**（`$XDG_STATE_HOME/cruise/new_session_draft.json`）: 新規セッションフォームの現在の内容（タスクの説明、組み込み `tdd` ワークフローを示す `__builtin__` センチネルを含む設定パス、作業ディレクトリ、リポジトリ、スキップするステップ）。TUI は変更を自動保存します。WebUI は **Save draft** をクリックしたときに保存します。フォームを再度開くとドラフトが復元され、その保存以降に送信していない入力も保持されます。
- **History**（`$XDG_STATE_HOME/cruise/history.json`）: 過去の新規セッション選択の記録です。設定ファイルごとに、ステップスキップセレクターで直近の選択を事前入力するため、および以前の作業ディレクトリ／設定の組み合わせを呼び出すために使われます。
