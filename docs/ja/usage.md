**言語:** [English](../en/usage.md) | [日本語](../ja/usage.md) | [简体中文](../zh/usage.md) · [目次](README.md)

<a id="webui"></a>
## WebUI

サーバー描画型ブラウザー UI を起動します。

```sh
cruise webui
```

このコマンドはフォアグラウンドで実行されます。代わりにバックグラウンドインスタンスを管理するには、次を使います。

```sh
cruise webui start --port 8484
cruise webui status
cruise webui status --json
cruise webui restart --port 8484
cruise webui stop
```

`start` と `restart` は、後述する同じサーバーオプションを受け付けます。オプションはサブコマンドの後に指定してください。正常なシャットダウンを完了できない場合、`stop --force` で強制終了できます。停止中のインスタンスを示す `status` は、ゼロ以外の終了ステータスを返します。WebUI の停止はセッション削除の要求ではありません。ライフサイクル用コマンドが管理するのはローカルインスタンスであり、システムサービスや自動起動監視ではありません。

フォアグラウンドとバックグラウンドのサーバーは、`XDG_DATA_HOME` が異なる場合でも、同じ state ディレクトリごとに 1 つのレジストリを共有します。起動したインスタンスを管理するには同じ `XDG_STATE_HOME` を使ってください。インスタンス実行中に `start` を再実行すると、その実行中インスタンスが報告されます。ホストまたはポートを変えるには `restart` が必要です。`status --json` は `running`、`stale`、`mode`、`origin`、`pid`、`version`、`started_at`、`log_path`、`outdated` などのフィールドを報告します。`status` は古いレコードを削除しませんが、`stop` は再利用された PID にシグナルを送らずに古いレコードを削除します。

`restart` は別の値を指定しない限り前回のホストとポートを再利用します。サーバーを起動するのは、元のサーバー起動時とは限らない、コマンドを呼び出した時点の作業ディレクトリです。開発用アセットを使う場合は `--webui-dir` も再指定し、相対パスを使う場合は意図したディレクトリから実行してください。停止と再起動は実行中の WebUI 操作のキャンセルを要求します。実行中のセッションは削除されず、中断状態になり、後から再開できます。強制終了すると正常なクリーンアップができない場合があります。

レジストリ、ロック、バックグラウンドログは `$XDG_STATE_HOME/cruise/webui/`（通常は `~/.local/state/cruise/webui/`）にあります。インスタンスレコードには制御資格情報が含まれるため、非公開に保ってください。バックグラウンドログは `webui.log` で、後続の起動時に 5 MiB を超えていれば `webui.log.1` にローテーションされます。Windows では `XDG_STATE_HOME` 未設定時の state ディレクトリは `%LOCALAPPDATA%\cruise\state` です。デタッチ起動、status、stop、restart は Unix と同様に Windows でも利用できます。Windows で正常な制御要求に失敗した場合は `stop --force` が必要になることがあります。

既定 URL は `http://127.0.0.1:8484/` です。次のフラグで変更できます。

- `--host <ADDR>` — bind するインターフェース（既定 `127.0.0.1`。WebUI に認証機能はないため、信頼できないネットワークに bind しないでください）。Merge PR ルートも同じ認証なしサーバーで提供され、画面上の確認はアクセス制御ではありません。信頼できないネットワークには絶対に WebUI を公開しないでください。
- `--port <PORT>` — 待ち受ける TCP ポート（既定 `8484`）。
- `-o`、`--open` — サーバー起動後に既定のブラウザーで WebUI を開きます（既定では開かれません。表示された URL に自分でアクセスしてください）。
- `--webui-dir <DIR>` — 開発用にディスク上の `templates/` と `static/` を提供します。例: `cruise webui --webui-dir webui`。テンプレートは各リクエストで読み直されるため、サーバーを再起動せずに編集して再読み込みできます。
- `Ctrl-C` — サーバーを停止します。実行中の操作はキャンセルされ（セッションは `Suspended` になります）、サーバーは約 10 秒以内に終了します。2 回目の `Ctrl-C` では直ちに終了します。

Graph タブは Mermaid を jsDelivr から読み込むため、ネットワーク接続が必要です。それ以外の WebUI タブはオフラインで動作します。

デスクトップではサイドバーのフッター、モバイルではヘッダーで表示言語を選択します。選択するとページが再読み込みされ、ブラウザーの `cruise_ui_language` cookie に約 1 年間保存されます。アプリケーション設定は変更されません。有効な cookie はサーバーの UI 言語より優先され、cookie がない場合はサーバープロセスの既定値が適用されます。

TSX コンポーネントの import は、import を記述したファイルからの相対パスです。たとえば `templates/components/session-row.tsx` は、兄弟ファイルのバッジを `./components/phase-badge` ではなく `./phase-badge` として import します。

<a id="usage"></a>
# 使い方

```sh
# Create a session (plan -> approve)
cruise plan "implement the feature"

# Create a session and generate the plan in the background
cruise --plan "implement the feature"

# Interview-style planning: answer one question at a time, then the plan is written (SDK backend + TTY + `interactive_planning: true`)
cruise plan --grill "implement the feature"

# Enable only browser and app E2E for the built-in test-first workflow
cruise plan -c builtin:tdd -f browser_e2e,app_e2e "task"

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

# Open the full interactive keyboard client
cruise

# List and manage sessions with the CLI selector
cruise list

# Review a session's changes, or just their summary
cruise diff <SESSION_ID>
cruise diff <SESSION_ID> --stat

# Remove closed/merged PR sessions and terminal no-PR sessions
cruise clean

# Open the TUI in a remote project
cruise ssh devbox --cwd /srv/project

# Run remote commands and forward stdin or root options unchanged
cruise ssh devbox --cwd /srv/project -- plan "implement the feature"
echo "implement the feature" | cruise ssh devbox --cwd /srv/project -- plan
cruise ssh devbox -- list --json | jq .
cruise ssh devbox -- run
cruise ssh devbox -- --plan "implement the feature"
cruise ssh devbox -- --plan "implement the feature" --repo owner/repository

# Legacy: positional input without a subcommand is treated as `cruise plan`
cruise "implement the feature"
```

サブコマンド `ssh` は、従来形式の入力が完全一致で `ssh` の場合、その入力を予約しています。タスク全体が `ssh` の場合は `cruise plan "ssh"` を使ってください。非表示の `mcp-bridge` エントリーポイントも入力が完全一致で `mcp-bridge` の場合に予約しています。このタスクには `cruise plan "mcp-bridge"` を使ってください。サブコマンド `tell` も予約されているため、そのテキストをタスクとして計画するには `cruise plan "tell"` を使います。

CLI と TUI は、**Action required**、**Plan ready**、**Completed**、**Failed** のワークフローイベントに対して、デスクトップ通知、サウンド、Slack webhook をベストエフォートでサポートします。デスクトップ通知には macOS または Linux のデスクトップセッションが必要です。サウンドは既定で無効です。配信に失敗してもワークフロー結果や終了ステータスは変わりません。3 つすべてを無効にするには `CRUISE_DISABLE_NOTIFICATIONS=1` を設定してください。設定と WebUI の動作については[通知](cli-reference.md#notifications)を参照してください。

<a id="herdr-integration"></a>
## herdr 連携

[herdr](https://herdr.dev) ペイン内（`HERDR_ENV=1` に加え `HERDR_PANE_ID` と `HERDR_BIN_PATH` が設定済み）では、TUI、`cruise plan`、`cruise run`、`cruise exec` が `"$HERDR_BIN_PATH" pane report-agent` を通じて、source `custom:cruise` / agent `cruise` としてライフサイクル状態を報告します。計画またはステップ実行中は `working`、回答・オプション選択・計画レビューを待つ間は `blocked`、それ以外は `idle` です。終了時に制御権は解放されます。報告はベストエフォートで、ワークフローの成功・失敗・終了ステータスには影響しません。無効にするには `CRUISE_DISABLE_HERDR=1` を設定してください。
herdr ペインから、実行中のセッションに指示するには `cruise tell <session-id> "instruction"` を使います。

<a id="tui-interactive-keyboard-client"></a>
## TUI（対話型キーボードクライアント）

引数なしで `cruise` を実行すると、CLI と WebUI に並ぶ第 3 のクライアントである Cruise 公式の対話型キーボードクライアントが開きます。既存の CLI 動作を維持しながら、WebUI のセッション管理ワークフローをターミナルで利用できます。

```sh
# Open the interactive client
cruise
```

一般的な流れ: `cruise` を起動して `n` を押し、Source、作業ディレクトリまたはリポジトリ、ワークフロー設定を選んでからタスクを入力します。通常の計画には `Ctrl-P`、grill 計画には `Ctrl-G`、入力をそのまま計画として使うには `Ctrl-U` を押します。または `Tab` で残りの質問に 1 つずつ回答し、最後に起動モードを選べます。Task の後の質問は Images、宣言済みワークフローフラグ（ある場合）、スキップするステップ（フラグで無効なステップは非表示）、Workspace（Directory セッションのみ）、必要に応じて Dirty tree、Launch です。`1` でセッションを確認または操作し、`3` で計画済みキューを **Run All** から実行します。Run All で `p` を押して共有並列数を編集し、正の整数を入力します。`Enter` で保存、`Esc` でキャンセルします。

TUI の利用には macOS、Linux、Windows 上の対話型 TTY が必要です。自動化インターフェースではありません。自動化、JSON 出力、CI には CLI を使ってください。GitHub を使うワークフローには外部の [`gh` CLI](https://cli.github.com/) が必要です。TUI はこれを同梱も代替もしません。Current-branch のみの作業には `gh` は不要です（GitLab リポジトリでは `glab` が必要です）。

<a id="screens-and-workflows"></a>
### 画面とワークフロー

TUI には 3 つのビューがあります。

- **Sessions** — 全体のセッション一覧を表示します。セッションを選ぶと **Info**、**Graph**、**Plan**、**Log**、**Diff** の詳細タブを表示します。[`cruise list` の操作一覧](sessions.md#cruise-list-actions)は list メニューの説明であり、TUI の操作全体ではありません。Info タブには宣言済みワークフローフラグとその on/off 値が表示されます。Graph タブにはノード一覧と選択ノードの依存関係・エッジの詳細が表示されます。フラグで無効なステップは、フィルターなしで表示する旧形式のバージョンなし graph チェックポイントを除き、非表示です。Markdown は解析・装飾表示されます。Ask や Option のプロンプト、Clean、worktree/current-branch の選択、Publish as Issue、PR リンクも扱います。
- **New Session** — 質問を 1 つずつ表示する対話形式でセッションまたは draft を作成します。回答済みの項目は上部に、残りの質問は下部に表示されます。順序は Source（ローカル Directory または GitHub リポジトリ）、作業ディレクトリまたはリポジトリ、ワークフロー設定、タスク、画像、ワークフローフラグ（設定が宣言している場合）、スキップするステップ（フラグで無効なステップは非表示）、workspace モード、dirty tree の許可（current-branch 実行のみ）、最後に起動モード（通常の計画、grill 計画、入力を計画として使用、draft として保存）です。先の回答で不要になった質問は省略されます。ローカル Directory セッションでは Workflow config の質問に Auto-detect が含まれ、CLI と同じ優先順位のファイル候補や組み込みワークフローを、設定したカテゴリと Others の表示設定に従って整理します。GitHub セッションの Auto-detect はクローン先リポジトリ内で解決されるため、呼び出し元ローカルの候補は含まれませんが、任意のパスは指定できます。どの質問からでも `Ctrl-P`、`Ctrl-G`、`Ctrl-U`、`Ctrl-S` で現在の回答からセッションを開始または draft 保存できます。Directory とパスの回答には補完があり、矢印キーで履歴を呼び出せます。draft と選択履歴については[新規セッションフォームの永続化](sessions.md#new-session-form-persistence)を参照してください。
- **Run All** — Planned または Suspended セッションを、ライブの並列実行、アプリ内の状態表示、ベル通知とともに実行します。`p` で共有 WebUI/TUI 並列数を編集できます。設定はアプリケーション設定に保存され、バッチ実行中にも変更できます。新しい上限は次のスケジューリング時点から適用され、すでに実行中の worker はキャンセルされません。1 つの TUI プロセス内では異なるセッションを並行実行できますが、同一セッションの重複作業は拒否されます。

PR と Issue の URL はテキストとして表示されます。Publish as Issue が成功した場合は、新しい issue の URL も自動的に開きます。専用の PR/Issue URL 操作では、macOS は `open`、Windows は `explorer.exe`、Linux は `xdg-open` で開きます。それ以外の Markdown リンクはテキストのままです。CLI 専用の `config` と `exec` は TUI 画面ではなく CLI コマンドで利用します。

New Session の回答は変更から 500 ms 後に自動保存されます。選択したセッションは、計画中またはフェーズが **Awaiting Input**、**Awaiting Approval**、**Planned** の間は **Plan** タブで、それ以外では **Info** タブで開きます。共有 Run All 並列数（アプリケーション設定に保存）を除き、ほかの画面状態は一時的です。手動で選択した詳細タブは、更新や計画変更を含め、TUI プロセスが続く間はセッションごとに保持され、既定のタブより優先されます。`ask_user` はモーダルを開かずセッションを一時停止し、セッションを **Awaiting Input** にして、質問をそのセッションの **Plan** タブだけに表示します。そのセッションで `o` を押すか **Answer Prompt** を選ぶと Plan タブを開いて詳細ペインにフォーカスします。`Enter` で編集を開始し、もう一度 `Enter` で送信し、`Esc` で下書きを残したまま編集を終了します。実行時の Option 要求は既存のモーダルキューに残ります。単独実行の Option は自動で開き、Run All ではキューバッジが表示されます。Delete、Discard、Reset to Planned、Publish as Issue、Stage Local Merge、Merge Locally & Cleanup、Merge PR（プレビューのモーダルで Squash、Merge、Rebase を選んで `Enter` を押す）、Clean、Run All / Cancel Run All、実行中に終了する操作のみ確認を求めます。Cancel、Approve、Run、Resume、Retry、Generate Plan、Open PR は直ちに実行されます。実行をキャンセルするとセッションは `Suspended` になり、計画をキャンセルすると元の状態と計画が復元されます。New Session では空白でないタスクまたは少なくとも 1 つの画像添付が必須で、GitHub ソースではリポジトリが必要です。空の作業ディレクトリは `.`、空のワークフロー設定は自動検出を意味します。通常終了、panic、SIGTERM、SIGHUP ではターミナル状態が復元されます。セッションエラーはアプリとセッション状態に保持されます。TUI を終了させるのはターミナル、root、イベントループの失敗だけです。

<a id="keyboard-map"></a>
### キーボード一覧

TUI はキーボードのみで操作します。キーは固定で、変更できません。

| キー | 操作 |
|---|---|
| `1` / `2` / `3` | Sessions / New Session / Run All を切り替えます |
| `n` | New Session を最初の質問 Source で開きます。タスク入力前に場所と設定を選びます |
| `r` | 更新します |
| `L` | UI 言語または `auto` を選び、アプリケーション設定に保存します |
| `?` | ヘルプを表示します |
| `q` / `Ctrl-C` | 終了します。実行中は確認後に作業をキャンセルします |
| `Ctrl-P` | 現在の回答から New Session を作成し、通常の計画を開始します |
| `Ctrl-G` | 現在の回答から New Session を作成し、grill 計画を開始します |
| `Ctrl-U` | 現在の回答から New Session を作成し、入力をそのまま計画として使います |
| `Ctrl-S` | New Session の回答を draft として保存します |
| `Tab` / `Shift-Tab` | 次／前の質問に移動します（候補が一致する場合はワークフロー設定の任意パスも含め、Tab で先にパスを補完します）。Sessions ではサイドバーと詳細ペイン間のフォーカスを切り替えます |
| 矢印キー / `j` / `k` / `PgUp` / `PgDn` / `Home` / `End` | リストや選択肢を移動します。対話画面では `Up` / `Down` でワークフロー設定候補を選択し、最近のディレクトリや `gh` リポジトリを呼び出すか、スキップするステップの一覧を移動します。テキスト質問では `Left` / `Right` / `Home` / `End` で文字列を編集し、`j` / `k` は文字として入力されます。テキスト以外の選択肢では `j` / `k` で移動します。Sessions ではフォーカス位置に応じて矢印キーの動作が変わります。サイドバーではセッションを切り替え（`Right` で詳細ペインへ移動）、詳細ペインでは Info、Plan、Log をスクロールするか Graph ノードを選択し、`Left` / `Right` で詳細タブを切り替えます |
| `[` / `]` | 詳細タブを切り替えます |
| `a` | 操作パレットを開きます |
| `p` | Run All で共有並列数を編集します。`Enter` で保存、`Esc` でキャンセルします |
| `e` | Run All で自動再キュー上限を編集します。`Enter` で保存、`Esc` でキャンセルします |
| `o` | Sessions では選択中セッションの Ask を Plan タブで開きます。それ以外では Option キューを処理するか、専用の PR/Issue URL を開きます |
| `f` | ログを追尾します |
| `c` | Sessions で Clean 可能なセッションと PR 終了済みセッションのクリーンアップ確認を開きます |
| `Ctrl-R` | 複数行 Edit Settings 入力で、保存のみ／保存して再生成を切り替えます |
| `Enter` | Plan Ask では回答の編集を開始または送信します。それ以外では New Session の回答を確定して次の質問へ進むか、セッション開始・draft 保存を行うか、タスク／画像入力に改行を挿入します |
| `Ctrl-Enter` | 現在の New Session の質問を進めます（タスク／画像エディターでは `Enter` が改行になるため必須）。複数行 Edit Settings では保存します（再生成が選択されていれば再生成も行います） |
| `Space` | 現在の選択肢または強調表示中のステップのスキップを切り替えます |
| `Esc` | Plan Ask の編集中は下書きを破棄せず編集を終了します。それ以外では前の質問に戻り、最初の質問では Sessions に戻ります |

<a id="layout-logs-and-process-behavior"></a>
### レイアウト、ログ、プロセス動作

- **120 列以上**では、固定 **34 列**のサイドバーと詳細ペインを使います。
- **80〜119 列**では、Sessions と Run All の 2 つのペインを縦に積みます（サイドバー／概要を詳細／ログの上に配置）。
- **80x24 未満**では、サイズ変更を促す通知を表示します。
- Sessions では、フォーカス中のペイン（サイドバーまたは詳細）に太枠を表示します。
- `NO_COLOR` に対応します。ラベルや状態は色だけで伝えません。
- アイドル時の更新はイベント駆動です。外部状態は 3 秒ごとにポーリングし、処理中は 100 ms の spinner を表示します。
- メモリー上の session log は直近 10,000 行まで、Run All ビューは直近 2,000 行までに制限されます。セッションごとの完全な実行出力は `$XDG_DATA_HOME/cruise/sessions/<session-id>/run.log`（既定では `~/.local/share/cruise/sessions/<session-id>/run.log`）に保持されます。
- TUI の標準入力を対話型子コマンドに渡すことはできません。子プロセスは TUI の stdin を共有しないため、そのようなコマンドは TUI 経由でプロンプトを表示できません。
- 別々の cruise プロセスから同じセッションを同時に変更することはサポートされません。1 つの TUI プロセス内では異なるセッションを並行実行でき、Run All は設定済みの run-all 並列数を使います。
- TUI は操作が必要なプロンプト、計画準備完了、実行完了、失敗に対する設定済みデスクトップ、サウンド、webhook 通知を非同期で配信します。描画をブロックせず、ワークフロー状態も変えません。3 つすべてを無効にするには `CRUISE_DISABLE_NOTIFICATIONS=1` を設定してください。

自動化、JSON（`cruise list --json`）、CI／非対話利用には引き続き CLI が標準クライアントです。既存のコマンドの使い方は[CLI リファレンス](cli-reference.md#cli-reference)を参照してください。

<a id="workflow-packages"></a>
## ワークフローパッケージ

`cruise workflow add <owner/repo[/path.yaml][@ref]>` は GitHub から `$XDG_CONFIG_HOME/cruise/workflows/`（既定は `~/.config/cruise/workflows/`）へインストールします。パスの指定がなく、ルートに `cruise-package.yaml` があればバンドルを選びます。それ以外の場合、単一ワークフローの既定は `cruise.yaml` です。明示的な YAML パスは単一ワークフローを選択しますが、予約されたベース名 `cruise-package.yaml` または `cruise-package.yml` で終わるパスは拒否されます。ref の既定値はリポジトリの既定ブランチで、コミットに固定されます。`--name` でパッケージ名（`[A-Za-z0-9_-]+`）を上書きできます。

バンドルマニフェストにはインストールするワークフローを明示的に列挙します。例:

```yaml
description: Review and implementation workflows
workflows:
  - name: review
    path: workflows/review.yaml
  - name: implement
    path: workflows/implement.yaml
```

名前は大文字小文字を区別せず一意で、英字、数字、`_`、`-` のみを使用できます。パスはリポジトリルートからの相対 `.yaml` または `.yml` パスで、一意でなければなりません。空の要素、`.`、`..` の要素、バックスラッシュを含まず、`cruise-package.yaml` または `cruise-package.yml` で終わることもできません。未定義のマニフェストキーは拒否されます。リポジトリ内の YAML 全件ではなく、列挙したワークフローだけがインストールされます。ネストされた `workflow_call` ステップと `prompt_file` の内容はインストール時にインライン化され、自己完結した YAML になります。

単一パッケージは `workflows/<name>.yaml` と `<name>.cruise-package.json` を保持します。バンドルは `workflows/<name>/<workflow>.yaml` と、ソース、ref、コミット、各インストール済み YAML の SHA-256 を記録する `.cruise-package-lock.json` を使います。バンドルの各メンバーは、通常の設定セレクターで `<name>/<workflow>.yaml` と表示されます。既存の単一パッケージはバンドルに移行されません。

`add` と `update` はソース、コミット、ワークフロー、コマンドをプレビューし、信頼されていないコマンドについて警告して `[y/N]` を確認します。TTY がない場合は `--yes` を指定してください（プレビューは引き続き表示されます）。インストール時にワークフローは実行されません。`update <name>` はパッケージ種別を変更せず記録済み ref を再解決し、`remove <name>` はパッケージを削除し、`list` は組み込みパッケージとインストール済みパッケージを表示します。バンドルの変更は一体としてステージングされます。`update` と `remove` はユーザー編集済みの YAML や追加ファイルを含むバンドルを拒否し、ユーザーの変更を保護します。

インストール済みパッケージの検査（YAML が変更されていないかを含む）には `cruise workflow doctor <name>` を使います。
