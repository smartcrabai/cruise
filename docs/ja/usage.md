**言語:** [English](../en/usage.md) | [日本語](../ja/usage.md) | [简体中文](../zh/usage.md) · [目次](README.md)

<a id="webui"></a>
## Daemon と WebUI

Cruise はユーザーおよび state directory ごとに 1 つの常駐 daemon を実行します。session command は daemon の client です。対象は `plan`、`--plan`、`draft`、`run`、`run --all`、`exec`、`list`、`tell`、`clean`、`diff` です。daemon が起動していなければ、これらの command が必要に応じて自動起動します。`cruise webui` も daemon の起動を確認し、WebUI の login URL を表示します。必要なら browser も開けます。

```sh
cruise webui
cruise webui --open
```

`cruise daemon` で daemon を管理します。

```sh
cruise daemon start
cruise daemon status --json
cruise daemon restart
cruise daemon stop --force
cruise daemon serve
```

`serve` は container や debugging 用の foreground 実行です。`start`、`restart`、`serve` は `--host`、`--port`、`--webui-dir`、`-o`/`--open` を受け付けます。既定 address は `127.0.0.1:8484` です。port を指定せず 8484 が使用中の場合、Cruise は警告を出して ephemeral port を使います。共有 host では `--port` または `CRUISE_DAEMON_PORT` を明示してください。どちらも `0` は ephemeral port を要求します。`cruise webui` の login URL には registry API token ではなく、5 分で期限切れになる一度だけ使える nonce が含まれます。ログインすると、API token とは別のランダムな in-memory session ID が HttpOnly、SameSite=Strict の `cruise_webui` cookie に設定され、daemon 終了時に無効になります。origin だけでは 401 が表示されます。`-o`/`--open` は 0600 の一時 HTML redirect file を開くため、URL は opener の command line に渡されません。loopback bind は `localhost`、`127.0.0.1`、`[::1]` の Host 名だけを任意の port で受け付け、DNS rebinding に使われる外部 Host 名を拒否しつつ SSH tunnel を許可します。non-loopback bind は平文 HTTP で配信されるため、信頼できる network 内、または TLS proxy の背後でのみ使用してください。

daemon registry と log は `$XDG_STATE_HOME/cruise/daemon/`（通常 `~/.local/state/cruise/daemon/`）にあります。`instance.json` は mode `0600` で、ランダムな API token を含みます。`daemon.log` に daemon log が記録されます。この directory は非公開に保ってください。起動時サービスのインストールも idle 時の自動終了もありません。registry record が削除されると daemon は終了します。従来の `cruise webui start|stop|restart|status` command は削除されました。`cruise daemon ...` を使ってください。
Detached daemon は 60 秒ごとに `daemon.log` を確認します。5 MiB を超えると、現在の log で `daemon.log.1` を置き換えて live log を truncate します。rotation は 1 世代だけ保持します。

v0.3.0 の分離 WebUI から更新する場合、旧 server が稼働中の間は `daemon start`、`restart`、`serve` と client の自動起動を拒否します。先に `cruise daemon stop` を実行してください。旧 server の停止では実行中の作業を cancel し、session は `Suspended` になります。`cruise run <id>` で再開できます。`cruise daemon status` は稼働中の旧 server を表示し、daemon 起動時には停止済みの旧 registry を削除します。

また、別の cruise process が runner として記録され実行中の session（更新時に進行中だった v0.3.0 の `cruise run` など）に対する run と planning を daemon は拒否します（HTTP 409、CLI は exit 1、state は変更しません）。v0.3.0 の background planning worker は runner を記録しないため検出できません。更新前に開始した background plan は完了を待ってから操作してください。

workflow 設定は CLI client 側で解決されます。working directory と `CRUISE_MODEL`、`CRUISE_PLAN_MODEL`、`CRUISE_SDK` などの workflow override 環境変数は各 request とともに送られ、その request にのみ適用されます。`PATH`、`GH_TOKEN`、provider credential、`CRUISE_FORGE`、`CRUISE_SKIP_AGENT_AUTH_CHECK`、`CRUISE_DISABLE_NOTIFICATIONS`、`CRUISE_COMMIT_COAUTHOR_*` などの process-level 変数は daemon の環境から取得されます。変更後は `cruise daemon restart` を実行してください。自動起動では workflow override 変数が daemon 環境から除外されます。明示的な `cruise daemon start` または `serve` ではその環境が保持されます。WebUI session は daemon の環境と home directory を working directory として使います。herdr の報告は引き続き client 側で行われます。

`cruise update` 後、daemon が busy の間は古い version のまま稼働します。CLI client は `cruise daemon restart` を idle 時に実行するよう案内して request を拒否し、WebUI も再起動まで古い version のままです。idle の daemon は次の client request で自動再起動します。

foreground command は daemon から進捗を stream します。Ctrl+C は操作をキャンセルします（`run` は `Suspended` になり、planning は元に戻ります）。terminal を閉じても daemon の作業は止まりません。`cruise run <session-id>` または WebUI から再接続できます。Ask と Option の prompt は terminal と WebUI に表示され、先に送信された回答が採用されます。非対話 client に回答待ち prompt がある場合、session は待機し、WebUI または `cruise run <session-id>` から回答できます。`cruise --plan` の planning も detached worker process ではなく daemon 内で実行されます。

session の変更はすべて daemon を経由するため、複数の CLI process と WebUI は安全に同時操作できます。claim により同じ session の作業重複を防ぎます。ローカル専用 command は `config`、`usage`、`update`、`install`、`workflow *`、`docs`、`ssh` とすべての `--dry-run` です。これらはローカル daemon に接続しません。

Graph tab は Mermaid を jsDelivr から読み込むため、ネットワーク接続が必要です。それ以外の WebUI tab はオフラインで動作します。desktop では sidebar footer、mobile では header から表示言語を選択します。選択するとページが再読み込みされます。

<a id="usage"></a>
# 使い方

```sh
# Create a session (plan -> approve)
cruise plan "implement the feature"

# Create a session and generate the plan in the background
cruise --plan "implement the feature"

# Interview-style planning: answer one question at a time, then the plan is written (SDK backend + TTY + `interactive_planning: true`)
cruise plan --grill "implement the feature"

# Enable applicable UI E2E for the built-in test-first workflow
cruise plan -c builtin:tdd -f e2e "task"

# 共通 UI E2E policy と framework ID 一覧を表示
cruise docs e2e
# React Native／Expo または Tauri の policy を表示
cruise docs e2e react-native
cruise docs e2e tauri

# Plan against a GitHub repository instead of a local directory (temporary clone)
cruise plan --repo owner/repository "implement the feature"

# Background planning from stdin
echo "implement the feature" | cruise --plan stdin

# Save the task as a draft (no plan yet); generate the plan later from `cruise list`
cruise draft "implement the feature"

# Execute (or resume) a pending session -- Planned, Running, Failed, or Suspended
cruise run

# Execute a config directly in the current directory (no plan, no worktree, no PR)
cruise exec "do this"

# Run one agent without writing a workflow YAML
cruise exec --instant "do this"

# TTY では bare `cruise` が対話型 session selector（`cruise list` と同じ）を開きます
cruise

# List and manage sessions with the CLI selector
cruise list

# Review a session's changes, or just their summary
cruise diff <SESSION_ID>
cruise diff <SESSION_ID> --stat

# Remove closed/merged PR sessions and terminal no-PR sessions
cruise clean

# 転送引数がない場合、リモートで `cruise list` を実行します
cruise ssh devbox --cwd /srv/project

# Run remote commands and forward stdin or root options unchanged
cruise ssh devbox --cwd /srv/project -- plan "implement the feature"
echo "implement the feature" | cruise ssh devbox --cwd /srv/project -- plan
cruise ssh devbox -- list --json | jq .
cruise ssh devbox -- run
cruise ssh devbox -- --plan "implement the feature"
cruise ssh devbox -- --plan "implement the feature" --repo owner/repository

# 位置引数が正確に "docs" の task は予約済みなので、cruise plan "docs" を使う
# Legacy: positional input without a subcommand is treated as `cruise plan`
cruise "implement the feature"
```

ワークフローイベント **Action required**、**Plan ready**、**Completed**、**Failed** の通知は、作業を開始した client（CLI または WebUI）にかかわらず daemon が送信します。サウンドと Slack webhook は常に daemon から配信されます。WebUI browser tab が接続されている場合、デスクトップ通知は browser notification として表示されます。WebUI browser が接続されていない場合、daemon がネイティブのデスクトップ通知を表示します（macOS/Linux のデスクトップセッション）。サウンドは既定で無効です。配信に失敗してもワークフロー結果や終了ステータスは変わりません。daemon の環境に `CRUISE_DISABLE_NOTIFICATIONS=1` を設定し、`cruise daemon restart` で再起動すると、daemon のデスクトップ通知、サウンド、webhook 配信が無効になります。WebUI のブラウザー通知は無効になりません。設定と WebUI の動作については[通知](cli-reference.md#notifications)を参照してください。

<a id="herdr-integration"></a>
## herdr 連携

herdr ペイン内（`HERDR_ENV=1` に加え `HERDR_PANE_ID` と `HERDR_BIN_PATH` が設定済み）では、`cruise plan`、`cruise run`、`cruise exec` がライフサイクル状態を報告します。報告はベストエフォートで、ワークフローの成功・失敗・終了ステータスには影響しません。無効にするには `CRUISE_DISABLE_HERDR=1` を設定してください。
herdr ペインから、実行中のセッションに指示するには `cruise tell <session-id> "instruction"` を使います。

<a id="workflow-packages"></a>
## ワークフローパッケージ

`cruise workflow add <owner/repo[/path.yaml][@ref]>` は GitHub から `$XDG_CONFIG_HOME/cruise/workflows/`（既定は `~/.config/cruise/workflows/`）へインストールします。パスの指定がなく、ルートに `cruise-package.yaml` があればバンドルを選びます。それ以外の場合、単一ワークフローの既定は `cruise.yaml` です。明示的な YAML パスは単一ワークフローを選択します。ref の既定値はリポジトリの既定ブランチです。`--name` でパッケージ名（`[A-Za-z0-9_-]+`）を上書きできます。

バンドルマニフェストにはインストールするワークフローを明示的に列挙します。例:

```yaml
description: Review and implementation workflows
workflows:
  - name: review
    path: workflows/review.yaml
  - name: implement
    path: workflows/implement.yaml
```

名前は大文字小文字を区別せず一意で、英字、数字、`_`、`-` のみを使用できます。パスはリポジトリルートからの相対 `.yaml` または `.yml` パスで、一意でなければなりません。リポジトリ内の YAML 全件ではなく、列挙したワークフローだけがインストールされます。

単一パッケージは `workflows/<name>.yaml` と `<name>.cruise-package.json` を保持します。バンドルは `workflows/<name>/<workflow>.yaml` を使います。バンドルの各メンバーは、通常の設定セレクターで `<name>/<workflow>.yaml` と表示されます。既存の単一パッケージはバンドルに移行されません。

`add` と `update` はソース、コミット、ワークフロー、コマンドをプレビューし、信頼されていないコマンドについて警告して `[y/N]` を確認します。TTY がない場合は `--yes` を指定してください（プレビューは引き続き表示されます）。インストール時にワークフローは実行されません。`update <name>` はパッケージ種別を変更せず記録済み ref を再解決し、`remove <name>` はパッケージを削除し、`list` は組み込みパッケージとインストール済みパッケージを表示します。`update` と `remove` はユーザー編集済みの YAML や追加ファイルを含むバンドルを拒否し、ユーザーの変更を保護します。

インストール済みパッケージの検査（YAML が変更されていないかを含む）には `cruise workflow doctor <name>` を使います。
