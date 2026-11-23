<a id="sessions-and-workspaces"></a>
# セッションとワークスペース

**Languages:** [English](../en/sessions.md)
<!-- sakoku-ignore-next-line -->
[日本語](../ja/sessions.md) · [简体中文](../zh/sessions.md) · [目次](README.md)

## 試験的な Manager の記録

`cruise manager` にはローカル Git リポジトリが必要です。記録は `$XDG_DATA_HOME/cruise/manager/`（通常は `~/.local/share/cruise/manager/`）に保存され、正規化された Git 共通ディレクトリをキーとするため、リンクされたワークツリー間で共有されます。`cruise manager status` で保存済みメッセージや提案、ゴールが表示されます。会話には `builtin:tdd` の既定値から設定された対話型 SDK と計画用モデルを使用し、command バックエンドは使えません。ユーザー入力はプロバイダーの呼び出し前に保存され、呼び出しが完了しなければ中断状態のまま残ります。SDK は一時ディレクトリで読み取り専用権限を要求されますが、これは完全なサンドボックスではありません。各提案は子セッションのワークフローを固定します。これはプロジェクトとユーザーの既定値および daemon 環境の上書きで解決した `builtin:tdd` で、Manager が Goal の単一の PR/MR を自ら公開するため、PR の自動作成、after-PR ステップ、`after_pr` と `merge` フラグ、PR 後のクリーンアップ、`force_exec` を除いたものです。session command と異なり、`cruise manager` は `CRUISE_MODEL` などクライアント側の workflow override 変数を転送しません。

`cruise manager` または `cruise manager status` でリポジトリの状態を確認できます。スクリプトでは `status` に `--json` を付けてください。`cruise manager say [TEXT]` の引数を省略すると、端末で複数行を入力するか、標準入力から読み込みます。`say` と `propose` は通常、完了まで待機し、ターンが完了しなければ 0 以外で終了します。`--no-wait` を指定するとターン ID を表示してすぐに終了します。Ctrl-C は待機のみを中断し、デーモンのターンは続行されます。

`cruise manager propose` は検証済みの提案プレビューを要求します。対象ブランチはローカルの `origin` 追跡 ref で解決できる必要があるため、必要に応じて先に fetch してください。`cruise manager approve` は実行可能なチェックを含む提案と契約内容を表示します。端末で `--yes` を指定しない場合は、確認のために `approve` を正確に入力してください。非対話モードでは `--yes` と `--revision N` の両方が必要です。指定したリビジョンは、確認した提案と一致する必要があります。承認すると、リポジトリと対象ブランチに対する永続的なアクティブ Goal が 1 つ登録され、デーモンのコーディネーターに適格な子セッションを非同期でスケジュールする権限が与えられます（承認操作中に同期実行されることはありません）。各子セッションはディスパッチ時の正確な統合済み Goal tip に固定して開始され、成功結果は逐次統合されます。その後、統合済みの正確な SHA を検証し、Cruise が Goal の単一の PR/MR を公開または復旧します。Manager はマージせず、検証済み head が承認済み対象ブランチへ人間によってマージされるのを待ちます。`cruise manager pause GOAL` と `cruise manager resume GOAL` はゴール単位の状態を記録し、`cruise manager answer` は一致する実行中の子セッションの質問や選択プロンプトに回答します。`cruise manager resume GOAL TASK` は、条件を満たす失敗または中断済みの子セッションを既存の worktree で続行します。`cruise manager retry GOAL TASK` は、安全性などの条件を満たす場合、アクティブなゴールで以前に失敗または中断した子セッション、または統合コンフリクトが安全に確認されたブロック中のゴールの完了済み子セッションについて、新しい試行を予約できます。依存関係、所有権、リポジトリなどの検査も必要です。`cruise manager retry GOAL` は全タスクが統合済みの適格なブロック中ゴールで検証をやり直し、以前の証拠を保持します。不確実な公開は再試行しません。未承認の修正案は破棄され、ゴールの監査履歴に保存されます。破棄された修正案は承認できません。`cruise manager corrective GOAL TASK` は条件を満たす確定したチェック失敗に対し、人が入力した指示を修正案として保存します。この時点では実行を許可しません。`cruise manager approve-corrective GOAL` はゴールと修正案を更新して表示し、`cruise manager approve` と同じ確認手順とリビジョンの規則を適用します。承認はその修正案と統合済み tip にのみ適用され、タスクを追加し、以前の検証証拠と承認済みチェックを保持します。修正案や tip が変更された場合は再確認が必要です。実行には Git 2.38 以降と認証済みの GitHub または GitLab の `origin` が必要です。

自由回答には `cruise manager answer GOAL PROMPT TEXT` を使うか、端末で複数行を入力します。選択式の場合、標準入力が端末ならメニューで選択します。端末でない場合は `--choice LABEL` に正確なラベルを指定し、テキスト入力が必要な選択肢に限り `--text TEXT` を指定します。修正指示は `--instructions TEXT`、端末のプロンプト、または標準入力で渡せます。

WebUI のサイドバーで **Manager** を選択し、一覧からリポジトリを選ぶか、デーモンが動作するホスト上の Git チェックアウトのパスを入力して **リポジトリを開く** を選びます。開くには既存の Manager 記録と、デーモンのホストからアクセスできるメインの Git チェックアウトの両方が必要です（リンクされたワークツリーのパスを入力した場合も同様です）。チェックアウトを開くだけでは記録の作成や作業の開始は行われません。会話、提案、ゴールは CLI と共有されます。リポジトリのページからメッセージを送信し、提案を作成できます。対象コミット、タスク、実行されるチェックを確認し、表示されたリビジョンを明示的に承認すると実行が許可されます。ページは自動更新され、状態に応じてゴールの一時停止と再開、タスクの再開と再試行、検証の再試行、修正タスクのプレビューと承認、保留中の Ask や Option への回答ができます。修正タスクはプレビューだけでは実行されません。Manager の子セッションのページは読み取り専用です。子セッションのゴールを含む読み取り可能な Manager 記録が見つかった場合に限り、**Manager で開く** からリポジトリの Manager ページへ戻れます。

<a id="runtime-file-layout"></a>
## ランタイムファイルの配置

Cruise は [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/latest/) に従い、ランタイムファイルを 3 つのディレクトリに分けて保存します。

| 種類 | パス |
|------|------|
| ユーザーのワークフロー YAML 設定（`workflows/*.yaml` / `*.yml`） | `$XDG_CONFIG_HOME/cruise/workflows/`（デフォルト: `~/.config/cruise/workflows/`） |
| ユーザーのワークフローデフォルト | `$XDG_CONFIG_HOME/cruise/default.yaml`（デフォルト: `~/.config/cruise/default.yaml`） |
| プロジェクトのワークフローデフォルト | `./.cruise/default.yaml` |
| 状態ファイル、WebUI インスタンス情報など | `$XDG_STATE_HOME/cruise/`（デフォルト: `~/.local/state/cruise/`） |
| daemon registry と log（`instance.json`、`daemon.log`） | `$XDG_STATE_HOME/cruise/daemon/`（デフォルト: `~/.local/state/cruise/daemon/`） |
| セッション、ワークツリー、一時的な `--repo` クローン | `$XDG_DATA_HOME/cruise/`（デフォルト: `~/.local/share/cruise/`） |
| Manager の記録 | `$XDG_DATA_HOME/cruise/manager/`（デフォルト: `~/.local/share/cruise/manager/`） |

Windows では、`XDG_*` 変数が未設定の場合、デフォルトは `%APPDATA%\cruise`（設定）、`%LOCALAPPDATA%\cruise\data`（データ）、`%LOCALAPPDATA%\cruise\state`（状態）です。これらの変数が設定されていない場合は上記の `~/` のデフォルトが適用され、`~\` は `~/` と同様に展開されます。

> **`~/.cruise/` から移行する場合:** 以前のバージョンではすべてのファイルを `~/.cruise/` に保存していました。`*.yaml` / `*.yml` は `~/.config/cruise/workflows/` に、`config.json` は `~/.config/cruise/` に、`sessions/` と `worktrees/` は `~/.local/share/cruise/` に、`history.json` と `new_session_draft.json` は `~/.local/state/cruise/` に移動してください。ワークツリーディレクトリを移動する場合は `git worktree move`（または `git worktree repair`）を使用します。
>
> **ワークフロー設定が以前 `~/.config/cruise/*.yaml` / `*.yml` にあった場合:** ユーザー設定の自動検出でワークフロー YAML を探す場所は、現在 `workflows/` サブディレクトリのみです。ファイルを `~/.config/cruise/workflows/` に移動してください。`~/.config/cruise/` 直下に古いワークフロー YAML ファイルが残っていると、cruise は警告を表示します。アプリケーション設定 `config.yaml` とワークフローデフォルト `default.yaml` はそのディレクトリに残しても警告の対象になりません。

<a id="session-lifecycle"></a>
## セッションのライフサイクル

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

   使用可能なプランがあれば、WebUI の **Fix**、**Ask**、**Verify** を **Awaiting Approval** または **Planned** で利用できます。Verify は menu action であり、Ask 内の `/verify` command ではありません。利用には PATH 上の `quint` と Alloy 用の Java 17 以降が必要です。

   **Approve** または **Execute now** を選ぶと、`-f/--flag` が指定されていない場合は、宣言済みワークフローフラグを先に選択します。次に **step skip selector** に、それらの値で有効になっているステップのみが表示されます。この実行でスキップするステップを切り替えて選択してください。選択内容は設定ファイルごとに保存され、同じ設定を使う次のセッションで事前選択されます。どちらかのセレクターをキャンセルすると、フラグの途中変更を保存したり、セッションを承認・実行したりせず、プラン承認メニューに戻ります。Approve 後の `cruise run` では、保存済みフラグと CLI の上書きを使い、フラグセレクターもスキップ選択の再確認も表示しません（[ワークフローフラグ](cli-reference.md#cruise-run)を参照）。

5. **`cruise run`** -- ID を省略すると、実行待ちのセッション（Planned、Running、Failed、Suspended）を選択し、該当するものが複数ある場合は選択を促します。`$XDG_DATA_HOME/cruise/worktrees/<session-id>/` にある Git ワークツリーを再利用（または作成）し、ワークフローを実行します。デフォルトでは、実行後にブランチコミットが残っている場合に PR を作成し、その後、設定された `after-pr` ステップを実行します。組み込みワークフローで PR を作成するのは `tdd` だけです。`-f after_pr` を指定すると、この PR 後処理を有効にします（マージは行いません）。`-f merge` を指定すると、同じ PR 後処理を 1 回実行した後、組み込みの同期的な squash merge を行います。`research` はこれらのフラグを宣言しますが PR を作成せず、`refactor`、`add-tests`、`add-formal-specs`、`security` はどちらのフラグも宣言しません。ワークフローレベルで `create_pr: false` にすると、自動コミット、push、PR 作成、`after-pr`、PR ベースのクリーンアップがスキップされ、通常のセッション、成果物、ワークツリーは保持されます。公開に失敗した場合は、リポジトリセッションとローカルワークツリーセッションのどちらも **PR Failed** になります。ワークフローを再実行せずに公開を再試行するには **Create PR** を使います。

   PR に関連付けられたセッションは、PR がクローズまたはマージされた後に `cruise clean` で削除できます。`create_pr: false` を設定した通常のセッションは、PR を待たずにセッション、成果物、ワークツリーを保持します。

   PR のない対象のローカルワークツリーセッションでは、セッションを保持したまま squash merge をステージするか、明示的にマージしてワークツリー、ブランチ、セッションをクリーンアップできます。

<a id="model-and-fallback-notices"></a>
## モデルとフォールバックの通知

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

`cruise exec` は、プランニング、ワークツリー作成、PR 作成を行わずにカレントディレクトリで実行します。成功するとセッションは削除されます。失敗したセッションは、明示的に再開または削除できるよう同じ ID で保持されます。一時停止または中断された exec セッションも ID で再開できます。`--instant` と `force_exec` にも同じ動作が適用されます。[exec コマンドリファレンス](cli-reference.md#cruise-exec)を参照してください。`force_exec` を 1 回だけ無効にするには `--no-force-exec` を使います。

失敗した exec セッションは `cruise list` に表示されますが、WebUI の一覧、`cruise run` の自動選択、`cruise run --all` の対象にはなりません。通常の Failed 用アクションの代わりに **Resume**、**Delete**、**Back** が表示されます。`cruise list --json` では、これらの行に `"exec": true` が追加され、`"plan_available": false` になります。通常の行の形式は変わりません。

WebUI の Directory ソースでは **Exec**、`force_exec` が有効な場合は **Execute** が表示されます。計画するには **Create draft** を使います。Running または Suspended の exec セッションはサイドバーに表示され、Suspended の場合はそこから再開できます。

<a id="cruise-list-actions"></a>
## `cruise list` のアクション

対話型の一覧は、セッションがなくても閉じません。以下の操作にはセッションの選択が不要です。

- **新規セッション**は、現在のディレクトリとそのワークフロー設定を使います。**対話形式で計画する**ではタスクを入力して生成されたプランを確認できます。**バックグラウンドで計画する**ではデーモンにプラン生成を任せ、完了を待たずに一覧へ戻ります。**下書きとして保存する**ではプランを生成せずにタスクを記録します。タスク入力をキャンセルするとセッションは作成されません。どちらの計画方法でも、設定で `force_exec` が有効であっても、自動でワークフローを実行することはありません。
- **クリーンアップ**は `cruise clean` と同じルールで動作します。確認の既定値は「いいえ」です。最後に表示されていたセッションを削除した場合も、一覧を更新して戻ります。
- **一括実行**は `cruise run --all` と同じ既定値で、対象となる Planned と Suspended のセッションを実行します。実行中のバッチがあれば、代わりにそのバッチへの接続を確認します。どちらの確認も既定値は「いいえ」です。実行中のバッチも対象セッションもなければ、その旨を表示して一覧へ戻ります。
- 一覧からは **終了**、Esc、Ctrl+C のいずれかで終了できます。サブメニューをキャンセルすると一覧へ戻ります。

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

Manager が管理する子セッションでは、一覧から利用できるのは **View Diff**（利用可能な場合）と **Back** のみです。子セッションの操作には `cruise manager` のサブコマンドを使用します。

`cruise list` には Verify は表示されません。Ask が既存の一覧にないのと同様です。

\* Open PR と Merge PR は、セッションに PR URL がある場合にのみ表示されます。Tell は実行中セッションの runner が稼働している場合にのみ表示されます。

\** ローカルマージアクションを利用できるのは、PR がなく、対象ブランチが記録されている適格なローカルワークツリーセッションです。マージインテントまたはコミットが記録されると Stage は利用できません。対象ブランチが記録されていない古いセッションも対象外です。

ワークツリーのブランチがある場合は、どのフェーズでも **View Diff** も利用できます。ブランチがすでに削除されている場合、差分の表示はエラーになります。

- **Generate Plan** -- `Draft` または `AwaitingInput` セッションのプランを生成します（`AwaitingApproval` に遷移します）。
- **Approve** -- プランを承認し、セッションを Planned フェーズに遷移させます。
- **Publish as Issue** -- 変更せずに `plan.md` を、解決済みリポジトリの GitHub Issue（または GitLab Issue）として公開し、その後ローカルセッションを削除します。
- **Run / Resume** -- セッションを実行または続行します。
- **Tell** -- 実行中セッションの次のエージェントプロンプトに指示を送信します。WebUI でも実行中に Tell を使えます。
- **Create PR** -- **PR Failed** セッションの公開を、ワークフローを再実行せずに再試行します。まず表示されたエラーと forge の認証を確認してください（GitHub の HTTPS 認証情報については `gh auth setup-git` で Git を設定できます）。セッションは公開の失敗またはキャンセル後も **PR Failed** のまま保持されます。致命的な after-PR エラーが発生すると **Failed** になります。成功するとセッションは完了し、PR が作成または再利用された場合にのみ、設定済み after-PR ステップが実行されます。
- **View Diff** -- セッションブランチの変更を確認してメニューに戻ります。比較とクリーンアップの制限については [`cruise diff`](cli-reference.md#cruise-diff)を参照してください。
- **Replan** -- フィードバックを入力してプランを再生成します。セッションは Planned フェーズのままです。
- **Open PR** -- `gh pr view --web` でセッションのプルリクエストをブラウザーで開きます。
- **Merge PR** -- セッションのプルリクエストを人が開始してマージします（WebUI でも利用できます）。最初に PR の状態、マージ可能性、レビュー判定、各 CI チェックが表示されます。次に `Squash`（デフォルト）、`Merge`、`Rebase` から選択して確認します。これは人の確認を必要とする GitHub 専用経路です。組み込みの `-f merge` は別の同期 squash 経路です。
- **Stage local merge** -- セッションを削除したりコミットしたりせずに、セッションブランチをベースチェックアウトのインデックスに squash します。ワークツリーの未コミット変更は、先にセッションブランチへコミットされます。
- **Merge locally & cleanup** -- 1 つのベースコミットに squash した後、ワークツリー、ブランチ、セッションを削除します。最初の squash では、記録された対象ブランチがローカルでチェックアウトされていること、追跡対象ファイルに変更のないベースであること、upstream より遅れていないことが必要です。Cruise は fetch も pull もしません。
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

`cruise list` または WebUI のセッション詳細画面がセッションを読み込む際、Running フェーズのセッションについて稼働状態を確認します。runner プロセスがすでに稼働していなければ、セッションは自動的に Suspended フェーズへ遷移します。これにより、クラッシュや強制終了の後にセッションが Running のまま永久に残ることを防ぎます。

通常の Suspended セッションは `cruise list` または WebUI から再開するか、Planned にリセットできます。`run --all` コマンドも Planned セッションとともに Suspended セッションを実行対象にします。

<a id="parallel-session-execution"></a>
Manager が管理する子セッションの再開には、上記の `cruise manager` サブコマンドを使用します。

## セッションの並列実行

Run All は、失敗したステップから同じ worker 内で、対象となる失敗を自動的に再試行できます。WebUI の設定または `cruise config --set-auto-requeue-max-attempts <N>` で 0 以上の上限を設定します。デフォルトは `0` で、再試行は無効です。分類されていないコマンドエラー、対象となるプロバイダーエラー、変更なしエラーは再試行されることがありますが、キャンセル、設定、認証、認可エラーは対象外です。各試行では通常の失敗通知も表示されます。失敗したステップがある完了済みの実行を、失敗として再分類することはありません。

セッションごとの保存カウントはバッチをまたいで使われる生涯上限です。手動の Run または Retry ではリセットされませんが、Reset to Planned ではリセットされます。Failed セッションはバッチ開始時の対象にならず、単独セッションの実行と exec セッションも自動再試行されません。上限を下げてもカウントは消去されません。カウントはセッション詳細、CLI の list JSON、バッチ概要に表示されます。

WebUI と CLI は `run --all` で複数セッションを同時に実行できます。すべての session mutation は常駐 daemon を経由するため、複数 CLI process と WebUI は同時に操作できます。claim により同一 session の作業重複を防ぎます。

- **WebUI**: 並列度は、選択されたアプリケーション設定ファイル（`$XDG_CONFIG_HOME/cruise/` 以下の `config.json` または `config.yaml`）にある `run_all_parallelism` で制御されます。`cruise config --set-parallelism <N>` で設定でき、デフォルトは `1` です。この設定は WebUI Run All のみに適用されます。
- **CLI**: 1 回の実行だけに適用する上書きには `cruise run --all --parallelism <N>` を指定します（デフォルトは `1`、つまり逐次実行）。保存済みの WebUI 設定は読み取られず、変更もされません。対話型端末で `--dry-run` を指定していない場合、CLI はスケジュールされた各セッションのタイトル、現在のステップ、状態、経過時間を示すライブダッシュボードも表示します。詳細なエージェント出力は `sessions/{id}/run.log` に保持されます。非 TTY と dry-run では通常のログ出力と最後の概要が使われます。

バッチスケジューラーの動作:
- Planned と Suspended のセッションを初期候補にします。
- 最大 `N` セッションを同時に起動します。

<a id="new-session-form-persistence"></a>
## 新規セッションフォームの永続化

WebUI は、次の 2 種類の状態をセッションをまたいで保持します。

- **Draft**: 新規セッションフォームの現在の内容（タスクの説明、設定パス、作業ディレクトリ、リポジトリ、スキップするステップ）。WebUI は **Save draft** をクリックしたときに保存します。フォームを再度開くとドラフトが復元されます。
- **History**: 過去の新規セッション選択の記録です。設定ファイルごとに、ステップスキップセレクターで直近の選択を事前入力するため、および以前の作業ディレクトリ／設定の組み合わせを呼び出すために使われます。
