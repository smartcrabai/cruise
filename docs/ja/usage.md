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

`start` と `restart` は、後述する同じサーバーオプションを受け付けます。オプションはサブコマンドの後に指定してください。正常なシャットダウンを完了できない場合、`stop --force` で強制終了できます。停止中のインスタンスを示す `status` は、ゼロ以外の終了ステータスを返します。WebUI の停止はセッション削除の要求ではありません。ライフサイクル用コマンドが管理するのはローカルインスタンスです。

`restart` は別の値を指定しない限り前回のホストとポートを再利用します。停止と再起動は実行中の WebUI 操作のキャンセルを要求します。実行中のセッションは削除されず、中断状態になり、後から再開できます。

WebUI のインスタンスレコード（`$XDG_STATE_HOME/cruise/webui/instance.json`、通常は `~/.local/state/cruise/webui/`）には制御資格情報が含まれるため、非公開に保ってください。

既定 URL は `http://127.0.0.1:8484/` です。次のフラグで変更できます。

- `--host <ADDR>` — bind するインターフェース（既定 `127.0.0.1`。WebUI に認証機能はないため、信頼できないネットワークに bind しないでください）。
- `--port <PORT>` — 待ち受ける TCP ポート（既定 `8484`）。
- `-o`、`--open` — サーバー起動後に既定のブラウザーで WebUI を開きます（既定では開かれません。表示された URL に自分でアクセスしてください）。
- `--webui-dir <DIR>` — 開発用にディスク上の `templates/` と `static/` を提供します。例: `cruise webui --webui-dir webui`。
- `Ctrl-C` — サーバーを停止します。実行中の操作はキャンセルされます（セッションは `Suspended` になります）。

Graph タブは Mermaid を jsDelivr から読み込むため、ネットワーク接続が必要です。それ以外の WebUI タブはオフラインで動作します。

デスクトップではサイドバーのフッター、モバイルではヘッダーで表示言語を選択します。選択するとページが再読み込みされます。

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

CLI と TUI は、**Action required**、**Plan ready**、**Completed**、**Failed** のワークフローイベントに対して、デスクトップ通知、サウンド、Slack webhook をベストエフォートでサポートします。デスクトップ通知には macOS または Linux のデスクトップセッションが必要です。サウンドは既定で無効です。配信に失敗してもワークフロー結果や終了ステータスは変わりません。3 つすべてを無効にするには `CRUISE_DISABLE_NOTIFICATIONS=1` を設定してください。設定と WebUI の動作については[通知](cli-reference.md#notifications)を参照してください。

<a id="herdr-integration"></a>
## herdr 連携

[herdr](https://herdr.dev) ペイン内（`HERDR_ENV=1` に加え `HERDR_PANE_ID` と `HERDR_BIN_PATH` が設定済み）では、TUI、`cruise plan`、`cruise run`、`cruise exec` がライフサイクル状態を報告します。報告はベストエフォートで、ワークフローの成功・失敗・終了ステータスには影響しません。無効にするには `CRUISE_DISABLE_HERDR=1` を設定してください。
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

- **Sessions** — 全体のセッション一覧を表示します。セッションを選ぶと **Info**、**Graph**、**Plan**、**Log**、**Diff** の詳細タブを表示します。[`cruise list` の操作一覧](sessions.md#cruise-list-actions)は list メニューの説明であり、TUI の操作全体ではありません。Ask や Option のプロンプト、Clean、worktree/current-branch の選択、Publish as Issue、PR リンクも扱います。
- **New Session** — 質問を 1 つずつ表示する対話形式でセッションまたは draft を作成します。順序は Source、作業ディレクトリまたはリポジトリ、ワークフロー設定、タスク、画像、ワークフローフラグ（設定が宣言している場合）、スキップするステップ、workspace モード、dirty tree の許可（current-branch 実行のみ）、最後に起動モードです。先の回答で不要になった質問は省略されます。
- **Run All** — Planned または Suspended セッションを、ライブの並列実行、アプリ内の状態表示、ベル通知とともに実行します。

PR と Issue の URL はテキストとして表示されます。Publish as Issue が成功した場合は、新しい issue の URL も自動的に開きます。それ以外の Markdown リンクはテキストのままです。CLI 専用の `config` と `exec` は TUI 画面ではなく CLI コマンドで利用します。

選択したセッションは、計画中またはフェーズが **Awaiting Input**、**Awaiting Approval**、**Planned** の間は **Plan** タブで、それ以外では **Info** タブで開きます。`ask_user` はモーダルを開かずセッションを一時停止し、セッションを **Awaiting Input** にして、質問をそのセッションの **Plan** タブだけに表示します。そのセッションで `o` を押すか **Answer Prompt** を選ぶと Plan タブを開いて詳細ペインにフォーカスします。`Enter` で編集を開始し、もう一度 `Enter` で送信し、`Esc` で下書きを残したまま編集を終了します。実行をキャンセルするとセッションは `Suspended` になり、計画をキャンセルすると元の状態と計画が復元されます。New Session では空白でないタスクまたは少なくとも 1 つの画像添付が必須で、GitHub ソースではリポジトリが必要です。

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
