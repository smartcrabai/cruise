**言語:** [English](../en/cli-reference.md) | [日本語](../ja/cli-reference.md) | [简体中文](../zh/cli-reference.md) · [目次](README.md)

<a id="cli-reference"></a>
# CLI リファレンス

`cruise manager` は試験的な対話型クライアントです。提案を明示的に承認すると永続的な Goal が登録され、非同期実行が許可されます。Manager は検証後に 1 つの PR/MR を公開し、人間によるマージを待ちます。利用前に[Manager のゴールと実行](sessions.md#試験的な-manager-の記録)を参照してください。

```
cruise [OPTIONS] [INPUT] [COMMAND]

Commands:
  plan         Create an implementation plan for a task
  draft        Save a task description as a draft without generating a plan
  run          Execute a planned session
  tell         Send an instruction to a running session
  list         List and manage sessions interactively
  manager      Open the experimental repository-scoped Manager client
  clean        Remove sessions with closed/merged PRs
  usage        Summarize locally recorded token usage
  docs         Print embedded common or framework-specific E2E policy
  config       Show or update application-level configuration (`config.json` or `config.yaml`)
  exec         Execute a workflow or an instant task in the current directory (no plan, no worktree, no PR)
  ssh          Run a cruise command on a remote host through OpenSSH
  daemon       Manage the resident daemon lifecycle
  webui        Ensure the daemon is running and print its browser UI login URL
  install      Install/update a provider CLI to its latest stable version or inspect managed status
  update       Update a managed native installation; `--status` lists recorded transactions read-only
  workflow     Manage workflows: list/eject built-ins, generate, interactively make or check YAML (`doctor`), install GitHub packages (`add`, `remove`, `update`)

Arguments:
  [INPUT]  Initial input (legacy: positional input without a subcommand uses `plan`)

Options:
      --plan <INPUT>           Create a plan in the background and return immediately (pass `stdin` to read piped stdin explicitly)
      --skip-planning          Use the input directly as the plan, skipping LLM-based plan generation
      --no-force-exec          Ignore `force_exec: true` in the workflow config and plan as usual
      --repo <OWNER/REPO>      Repository (owner/repository, group/sub/project, or full URL; GitHub or GitLab) to clone into a temporary directory for planning and execution
      --image <PATH>           Attach an image file (png/jpg/jpeg/webp/gif) to the planning input; can be repeated
      -f, --flag <NAME[,NAME...]> Enable named workflow flags; NAME=on|off|true|false also works (repeatable)
```

`docs` という正確な名前は subcommand として予約されています。位置引数が正確に `docs` となる task を計画するには `cruise plan "docs"` を使ってください。
<a id="cruise-daemon"></a>
## `cruise daemon` and `cruise webui`

```sh
cruise daemon start [--host <ADDR>] [--port <PORT>] [--webui-dir <DIR>] [-o|--open]
cruise daemon stop [--force]
cruise daemon restart [--host <ADDR>] [--port <PORT>] [--webui-dir <DIR>] [-o|--open]
cruise daemon status [--json]
cruise daemon serve [--host <ADDR>] [--port <PORT>] [--webui-dir <DIR>] [-o|--open]
cruise webui [-o|--open]
```

`start` と `restart` は background、`serve` は container や debugging 用の foreground 実行です。host、port、WebUI directory、browser を開く option は `start`、`restart`、`serve` で使えます。`cruise webui` は daemon の起動を確認し、login URL を表示します。daemon の `start`、`restart`、`serve` も login URL を表示します。必要なら browser も開けます。従来の `cruise webui start|stop|restart|status` は削除されました。`cruise daemon ...` を使ってください。

既定 address は `127.0.0.1:8484` です。port を指定せず 8484 が使用中の場合、Cruise は警告を出して ephemeral port を使います。共有 host では `--port` または `CRUISE_DAEMON_PORT` を明示してください。どちらも `0` は ephemeral port を要求します。`cruise webui` の login URL には registry API token ではなく、5 分で期限切れになる一度だけ使える nonce が含まれます。ログインすると、API token とは別のランダムな in-memory session ID が HttpOnly、SameSite=Strict の `cruise_webui` cookie に設定され、daemon 終了時に無効になります。origin だけでは 401 が表示されます。`-o`/`--open` は 0600 の一時 HTML redirect file を開くため、URL は opener の command line に渡されません。loopback bind は `localhost`、`127.0.0.1`、`[::1]` の Host 名だけを任意の port で受け付け、DNS rebinding に使われる外部 Host 名を拒否しつつ SSH tunnel を許可します。non-loopback bind は平文 HTTP で配信されるため、信頼できる network 内、または TLS proxy の背後でのみ使用してください。

registry と log は `$XDG_STATE_HOME/cruise/daemon/`（通常 `~/.local/state/cruise/daemon/`）にあります。`instance.json` は mode `0600` でランダムな API token を含み、`daemon.log` に daemon log が記録されます。起動時 service のインストールも idle 時の自動終了もありません。registry record が削除されると daemon は終了します。

v0.3.0 の分離 WebUI から更新する場合、旧 server が稼働中の間は `daemon start`、`restart`、`serve` と client の自動起動を拒否します。先に `cruise daemon stop` を実行してください。旧 server の停止では実行中の作業を cancel し、session は `Suspended` になります。`cruise run <id>` で再開できます。`cruise daemon status` は稼働中の旧 server を表示し、daemon 起動時には停止済みの旧 registry を削除します。

また、別の cruise process が runner として記録され実行中の session（更新時に進行中だった v0.3.0 の `cruise run` など）に対する run と planning を daemon は拒否します（HTTP 409、CLI は exit 1、state は変更しません）。v0.3.0 の background planning worker は runner を記録しないため検出できません。更新前に開始した background plan は完了を待ってから操作してください。

`plan`、`--plan`、`draft`、`run`、`run --all`、`exec`、`list`、`tell`、`clean`、`diff` は daemon client で、必要に応じて自動起動します。`config`、`usage`、`update`、`install`、`workflow *`、`docs`、`ssh`、すべての `--dry-run` はローカル専用です。workflow config は CLI client 側で解決され、working directory と `CRUISE_MODEL`、`CRUISE_PLAN_MODEL`、`CRUISE_SDK` など許可リスト内の workflow override は request ごとに適用されます。`PATH`、`GH_TOKEN`、provider credential、`CRUISE_FORGE`、`CRUISE_SKIP_AGENT_AUTH_CHECK`、通知や commit coauthor の設定など process-level 変数は daemon 環境から取得されます。変更後は `cruise daemon restart` を実行してください。自動起動では workflow override 変数を daemon 環境から除去し、明示的な `daemon start` と `serve` では保持します。WebUI request は daemon の環境と home directory を working directory として使います。

異なる Cruise version の daemon が idle なら、次の client request が自動再起動します。busy の場合は idle になってから `cruise daemon restart` を実行するよう案内されます。`cruise update` の後、次の command は idle daemon を再起動します。foreground command は進捗を stream し、Ctrl+C で操作をキャンセルします（`run` は `Suspended`、planning は元の状態に戻ります）。terminal を閉じても daemon の作業は止まりません。`cruise run <session-id>` または WebUI から再接続できます。Ask と Option prompt は terminal と WebUI に表示され、先に送信された回答が採用されます。非対話 client で回答待ちの場合、session は待機し、WebUI または `cruise run <session-id>` から回答できます。`cruise --plan` の計画も daemon 内で実行します。session mutation は daemon を経由し、claim により同じ session の重複実行を防ぎます。

<a id="cruise-install"></a>
## `cruise install`

`cruise install jcode [--force]` は managed jcode CLI をインストールまたは更新します。各呼び出しでリリースメタデータを確認し、同じバージョンの正常な managed installation は再利用し、別バージョンなら更新します。`--force` は最新バージョンを再ダウンロードします。`cruise install --status` は読み取り専用でオフライン動作し、記録されたインストール済みバージョンを表示します。jcode CLI は `CRUISE_JCODE_CLI_PATH`、Cruise 管理 installation、PATH の順に解決されます。認証は jcode provider が管理します。詳細は[管理対象 jcode CLI](installation.md#managed-provider-clis)を参照してください。

<a id="cruise-docs-e2e"></a>
## `cruise docs e2e`

```sh
cruise docs e2e
cruise docs e2e react-native
cruise docs e2e jetpack-compose
cruise docs e2e flutter
cruise docs e2e tauri
cruise docs e2e slint
cruise docs e2e fyne
cruise docs e2e browser-extension
```

Cruise に組み込まれた UI E2E policy を表示します。framework ID を省略すると共通 policy と利用可能な ID 一覧を表示し、指定すると該当 framework の policy を表示します。出力は read-only で実行中の binary に同梱され、Cruise の version にひも付き、offline で利用できます。実行時の file、config、credential は読み込まず、network request や telemetry も行いません。正規の source は [docs/e2e/common.md](../e2e/common.md) と対応する `docs/e2e/<framework>.md` です。

| Framework ID | 適用範囲 |
|---|---|
| `browser` | Browser ベースの web UI |
| `swiftui` | SwiftUI と Apple native iOS/macOS UI |
| `jetpack-compose` | Android native Jetpack Compose UI |
| `react-native` | React Native（Expo を含む） |
| `flutter` | Flutter UI |
| `electron` | Electron desktop UI |
| `tauri` | Tauri desktop UI |
| `slint` | Slint desktop / embedded UI |
| `fyne` | Fyne native Go UI |
| `browser-extension` | Browser extension |

旧 ID の `android` は `jetpack-compose` に置き換えられました。`android` は alias や fallback ではありません。Jetpack Compose を使わない Android UI は未掲載であり、共通 policy に従います。repository に既にある適切な tool のみを使い、不足する capability を報告してください。新しい default tool は導入しないでください。その他の未掲載 native target も非対応を意味するものではなく、同じ no-new-default rule に従います。

<a id="cruise-workflow-generate"></a>
## `cruise workflow generate`

```
cruise workflow generate <DESCRIPTION> --name <NAME> [--user] [--config <PATH>]
```

説明文から新しい workflow YAML の草案を作成します。ファイルは `./.cruise/<NAME>.yaml` に保存されます。`--user` を指定するとユーザーワークフローディレクトリ（`~/.config/cruise/workflows/`）に保存されます。`NAME` に使えるのは英字、数字、`-`、`_` のみです。

- 検証済み YAML は backend の返答どおりに保存されます。同名ファイルがすでにある場合は上書きせず、その場合 backend も呼び出しません。
- 生成した workflow は実行されず、session も作成されません。
- 検証はコマンドの安全性や workflow の成功を保証しません。選択した agent backend は通常どおりの権限で動作するため、生成中に repository を変更することがあります。実行前に YAML を確認してください。

対話形式で草案を作成・修正するには、代わりに `cruise workflow make` を使ってください。

<a id="cruise-workflow-make"></a>
## `cruise workflow make`

```
cruise workflow make --name <NAME> [--user] [--config <PATH>] [--from <SELECTOR>] [DESCRIPTION]
```

対話形式で会話しながら単一 workflow YAML を作成します。stdin と stdout の両方が terminal でなければなりません。非対話形式の草案には `workflow generate` を使ってください。任意の説明文があれば最初のリクエストとして送信されます。`--config` は backend 設定を選び、`--from` は参照専用の workflow（例: `builtin:tdd` またはファイルパス）を指定します。Cruise は参照 workflow を書き換えませんが、sandbox 化されていない command backend が変更する可能性はあります。出力先は起動時に固定され、`./.cruise/<NAME>.yaml` または `--user` 指定時のユーザーワークフローディレクトリになります。既存の出力先は上書きされず、予約済みプロジェクト名 `default` は使えません。

リクエストを入力して草案を作成または修正します。assistant は草案を変更せず質問することもできます。`/show` で現在の草案を確認し、`/accept` で **Write workflow**、**Continue editing**、**Cancel** を選択します。Write を選んだときだけ、出力先に作成します。workflow は実行されず、session も作成されません。

各リクエストで候補を検証し、最大 3 回修正を試みます。すべて失敗した場合、直前の草案は変更されません。検証はコマンドの安全性を保証しないため、実行前に結果を確認してください。

草案と `conversation.jsonl` は書き込みまたはキャンセル後も `$XDG_DATA_HOME/cruise/workflow-make/<id>/`（通常 `~/.local/share/cruise/workflow-make/<id>/`）に保持されます。log にはリクエストも記録されます。これらのディレクトリは `cruise clean` では削除されません。不要になったら手動で削除してください。

command backend は sandbox 化されていません。backend が変更した可能性がある場合は repository を確認してください。

<a id="cruise-workflow-doctor"></a>
## `cruise workflow doctor`

```sh
cruise workflow doctor [--json] [--strict] [-f NAME[=on|off]]... [TARGET]...
cruise workflow doctor --all [--json] [--strict]
```

workflow YAML を静的に検査します。実行、session／worktree／プロジェクト設定の作成はしません。target を指定しない場合、`cruise.yaml`、`cruise.yml`、`.cruise.yaml`、`.cruise.yml` と、`.cruise/default.yaml` を除く `.cruise/` 内の YAML ファイルを検査します。`--all` はユーザーワークフローと全組み込み workflow も検査します。target または `-f` とは併用できません。

target にはパス、`builtin:<name>`、`__builtin__`（既定の組み込み workflow）、ユーザーワークフローまたは組み込み workflow の短い名前を指定できます。同じ短い名前を持つユーザーワークフローと組み込み workflow がある場合は曖昧になるため、明示パスまたは `builtin:<name>` を使ってください。複数 workflow の検査には target を繰り返します。全 target に適用する flag は `-f` を繰り返すか、カンマ区切りの割り当てを使います（例: `-f review,merge=off`）。target が宣言していない flag はその target のエラーです。`-f` がなければ flag はオフです。

ローカル prompt／workflow 参照、設定、routing target、retry 予算、flag 名、graph 到達可能性を検査します。実行時と同じ既定値および環境 override を使います。GitHub workflow 参照は通常どおり解決され、ネットワークアクセスが必要な場合があります。backend、model、認証、provider 接続、コマンドの安全性、実行時に決まる template 変数は検証しません。

選択した flag のもとで到達不能なステップは、必ずしも欠陥とは限らないため警告になります。開始点から通常終了に到達できず、到達可能な cycle が通常終了に至らない場合はエラーです。

人が読むレポートは stdout に出力されます。`--json` は `schema_version: 1`、`ok`、`exit_code`、`summary`（`checked`、`passed`、`failed`、`errors`、`warnings`）、順序付き `targets` を含む単一 JSON object を出力します。各 target には target、kind（`project`、`user`、`builtin`、`file`）、package status、path、status（`ok`、`warning`、`error`、`unresolved`）、step 数、retry 上限、宣言済み flag、順序付き診断が含まれます。診断には `level`、`code`、`stage`、nullable な `step`、`message` が含まれます。自動処理では message の文言ではなく `code` と `step` を使ってください。

検査成功時（警告は許容）は終了 status `0`、エラーまたは `--strict` と警告の組み合わせでは `1`、未解決 target または target なしでは `2` です。status `2` は `1` より優先され、引数の使用方法エラーも `2` で終了します。

<a id="cruise-ssh"></a>
## `cruise ssh`

```
cruise ssh [OPTIONS] <DESTINATION> [-- <CRUISE_ARGS>...]
```

`cruise ssh` は OpenSSH 経由でリモートの `cruise` にコマンドを渡して実行します。転送する引数がない場合、PTY が利用可能ならリモートの `cruise` は `cruise list`（対話型 session selector）を実行します。接続先には OpenSSH の `[user@]host` または `~/.ssh/config` alias を指定します。port、identity file、agent、ProxyJump にはその config を使ってください。

Options:

```
      --cwd <PATH>                 Change to this remote directory before starting cruise
      --cruise-bin <PATH>          Remote cruise executable or path [default: cruise]
      --tty <auto|always|never>    PTY policy: follow local TTYs, force a PTY, or disable one [default: auto]
```

`--` の後はすべて同じ順序でリモートの cruise に渡され、そこで解析されます。`auto` ではローカル TTY が PTY を要求します。pipe または redirect された stream では要求されず、`cruise ssh host -- list --json` などのスクリプト利用に適しています。`--tty always` と `--tty never` で選択を上書きできます。

実行境界はリモートホストです。セッション、設定、jcode 認証、worktree、repository clone はリモート側の XDG ディレクトリに保存され、ローカルの `cruise` コマンドとは共有されません。`--cwd`、`--config`、`--image` などのパス引数はリモートホスト上で解釈されます。`#12` などの issue 入力はリモートホストの `gh` と認証を使って解決されます。Cruise がローカルファイル、credential、環境変数を暗黙にアップロードしたり、リモート binary をインストールしたり、repository を同期したりすることはありません。WebUI はローカル session client のままで、リモートセッションを表示しません。

<a id="cruise-1"></a>
## `cruise`

```
cruise
```

引数なしで `cruise` を実行すると、対話型 terminal では `cruise list` が開きます。pipe 入力では従来どおり位置入力による planning を行います。[`cruise list`](#cruise-list) では selector と JSON output を説明します。

<a id="cruise-plan"></a>
## `cruise plan`

```
cruise plan [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description

Options:
      --issue <N>                  Use GitHub issue N as the task
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ or builtin:<name> for a built-in workflow (see Config File Resolution)
      --dry-run                    Print the plan step without executing it
      --no-force-exec              Ignore force_exec: true and plan as usual
      --skip-planning              Use the input directly as the plan, skipping LLM-based plan generation
      --grill                      Interview-style planning: the agent asks one question at a time, then writes the plan (requires the SDK backend and a TTY; conflicts with --skip-planning)
      --no-interactive-planning    Disable interactive planning tools for this session; the agent writes plan.md directly (conflicts with --grill)
      --repo <OWNER/REPO>          Repository (owner/repository, group/sub/project, or full URL; GitHub or GitLab) to clone into a temporary directory for planning and execution
      --pr <NUMBER>                Address reviews on an existing GitHub pull request
      --rate-limit-retries <N>     Maximum number of retries per LLM call (SDK fallback policies also use it for retryable 4xx, 5xx, and network failures and fallback switching) [default: 5]
      --image <PATH>               Attach an image file to the planning input; can be repeated
      -f, --flag <NAME[,NAME...]> Enable named workflow flags; NAME=on|off|true|false also works (repeatable)
```

**GitHub issue を入力として使う。** `cruise plan '#12'`、`cruise plan --issue 12 'additional instructions'`、または 1 行目を `#12` だけにして、後続行に追加指示を書く方法があります。1 行目には issue 参照のみを記述してください。root の位置引数、`--plan`、`draft`、WebUI の New Session task field でも同じ構文が使えます。`--issue` を受け付けるのは `plan` と `draft` のみであり、root コマンドや `exec` では使えません。issue は `gh` と現在の repository origin、または指定された `--repo` を使って検索します。GitLab issue input は未対応です。issue のタイトル、本文、label、コメントを task input に加え、対応する GitHub 画像はユーザー画像より先に session attachment として保存されます。issue 番号は branch 名と PR の close reference に含まれます。issue input は `force_exec` を無効化します。取得した issue 内容は信頼できない入力であり、prompt injection を含む可能性があります。

Git 管理されていないディレクトリでは警告を出してその場で計画しますが、`cruise run` には引き続き git repository が必要です。

`--skip-planning` を指定すると LLM を呼び出さず、入力をそのまま `plan.md` に書き込みます。対話型 TTY 上のフォアグラウンド `cruise plan --skip-planning` では引き続き承認メニューが開きます。stdin が非 TTY の場合と、バックグラウンド root `cruise --plan --skip-planning` の場合は Planned に自動承認されます（`force_exec: true` なら、issue input または他の opt-out がない限り workflow が直接実行されます。明示的に外すには `--no-force-exec` を指定します）。空または空白だけの入力は拒否されます。計画を自分で用意し、Cruise にはその実行だけを任せる場合に使います。

`--grill` を使うと、SDK 計画が interview 形式になり、`plan.md` を書く前に agent が 1 つずつ質問し、回答を提案します。既定の `jcode` を含む SDK backend、対話型 terminal、`interactive_planning: true` が必要です。`--skip-planning` とは併用できず、初回計画にのみ適用されます。

`--no-interactive-planning` を指定すると、workflow config が `interactive_planning: true` でも SDK 計画は直接ファイルに書き込む方式を使います。`--grill` とは併用できません。

**組み込み workflow flag。** `tdd` と `research` は `after_pr` と `merge` を宣言し、明示的に有効化しない限り全 flag はオフです。`research` は `create_pr: false` の read-only workflow なので、これらの after-PR flag は受け付けられますがスキップされます（下記参照）。`refactor`、`add-tests`、`add-formal-specs`、`security` はどちらの flag も宣言せず、after-PR step もありません。これは `--no-force-exec` を指定しても同じです。`--no-force-exec` は通常の plan/session 経路を選びますが、その経路では対象の commit がある場合に worktree で PR を作成します。`pr-review-fix` は flag を宣言しません。`tdd` と `refactor` は追加で単一の `e2e` flag と `formal_methods` を宣言します。browser／app 別の flag はありません。たとえば `cruise plan -c builtin:tdd -f e2e "task"` は E2E を有効にし、`-f e2e,merge` は after-PR 自動処理とネイティブ同期 squash merge を組み合わせます。`merge` は `after_pr` と同じ after-PR 前処理を 1 回実行し、`after_pr` 単独では merge しません。

- `e2e` は、適用可能な web／mobile／desktop UI の E2E を、repository にある適切な既存 tool を使って有効にします。prompt は `cruise docs e2e` と `cruise docs e2e <framework>` で組み込みの共通 policy と該当 framework policy を参照します。正規の source は `docs/e2e/` です。ID のない target（未掲載の native target を含む）も非対応ではありません。既存の適切な tool のみを使い、不足する capability を報告し、新しい default tool を導入しないでください。実際の browser／native UI screenshot を撮り、pixel 単位で目視確認して、assertion を含む deterministic な E2E test を保存します。`tdd` では 1 つの `e2e` step が `verify-review-pass` の前に実行されます。`refactor` では `prepare-e2e`（`refactor/e2e-preparation.md` を作成）、既存の `verify-refactor-pass` review、その後の `e2e-final-check`（`refactor/e2e-verification.md` を作成）が実行され、refactor の E2E 2 step はどちらもこの flag が必要です。対象外の機能は not applicable、必要な capability がない場合は blocker として記録し、実行可能な作業は完了して tool／version／setup／result を明記します。
- `after_pr` は merge なしで after-PR 自動処理を有効にします。
- `merge` は同じ after-PR 前処理に加えて、最後にネイティブの同期 squash merge を有効にします。GitHub では base から遅れている PR や競合している PR を、merge 前に自動で同期・復旧します。GitLab ではこの復旧は行いません。auto-merge を有効化せず、policy を回避せず、merge queue も使いません。
- `formal_methods` は workflow レベルの実行可能な Quint、Alloy、Gherkin 仕様と CI を有効にします。初期 plan mode ではありません。

`research` は `create_pr: false` の read-only workflow です。`merge` は PR を作成せず、no-PR policy を上書きしません。PR が作成されないため opt-in の after-PR 処理を利用できない場合（`research`、`exec`、current-branch run、commit のない run など）、Cruise は理由を表示してその処理をスキップします。

`--repo <owner/repository>` を指定すると、現在の directory ではなく GitHub または GitLab repository を対象にします。repo session は常に Worktree mode で実行され、アクセスには引き続き `gh` / `glab` が必要です。`create_pr: false` の場合、自動 commit、push、PR 作成、`after-pr`、PR ベースの cleanup をスキップします。新しい branch commit がない成功 run では PR 作成を試みず完了しますが、既存の branch commit は PR の対象になります。repo session とローカル worktree session のいずれも公開失敗時は **PR Failed** となり、workflow をやり直さず workspace を保持して **Create PR** による復旧を可能にします。`--repo` はバックグラウンド計画（`cruise --plan "task" --repo owner/repository`）でも使えます。自由入力の `owner/repository` も受け付けます。

**既存 GitHub PR のレビュー修正。** repository 内から `cruise plan --pr 12`、または `cruise plan --repo owner/repo --pr 12 "focus on tests"` を実行し、表示された場合は生成計画を承認してから `cruise run <session-id>` を実行します。任意の位置引数テキストは追加指示になります。このモードは stdin を読まず、LLM に計画を生成させません。WebUI の New Session form でも GitHub repository の PR 番号を指定できます。認証済み `gh` が必要です。PR は Open で、同じ repository 内に空でない head branch を持つ必要があります。draft PR は対応しますが、fork PR と GitLab merge request は対象外です。`--grill` と `--image` は `--pr` と併用できません。

既定 workflow はプロジェクト workflow ではなく `builtin:pr-review-fix` です。カスタム `-c` workflow では PR 処理を有効にし、`reviewers: all` を指定する after-PR `github-review` ステップを含める必要があります。組み込み workflow は flag を宣言せず、PR の作成や merge はせずに review に対応します。review loop は修正を commit／push し、対象 thread に返信または resolve します。失敗した session は再試行可能です。PR session では Publish as Issue と Replan は利用できません。

<a id="cruise-draft"></a>
## `cruise draft`

```
cruise draft [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description (omit to prompt interactively; reads from stdin when piped)

Options:
      --issue <N>                  Use GitHub issue N as the task
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ or builtin:<name> for a built-in workflow (see Config File Resolution)
```

LLM を呼び出さずに入力を `Draft` session として保存します。`cruise list` で **Generate Plan** を選べば後から計画を作成できます。すぐに記録したいが、まだ計画を始めたくない（または計画の費用をかけたくない）アイデアに便利です。

<a id="cruise-run"></a>
## `cruise run`

```
cruise run [OPTIONS] [SESSION]

Arguments:
  [SESSION]  Session ID to execute (if omitted, picks from pending sessions)

Options:
      --all                        Run all planned or suspended sessions (live dashboard on interactive terminals for non-dry runs)
      --parallelism <N>            Max number of sessions `--all` executes concurrently (no flag default; 1 when omitted, must be >= 1, requires `--all`)
      --auto-requeue-max-attempts <N>  Maximum automatic retries per session (requires `--all`)
      --max-retries <N>            Maximum number of times a budgeted graph transition may be traversed (no flag default; falls back to the workflow config's top-level `max_retries`, else 3)
      --rate-limit-retries <N>     Maximum number of retries per step (SDK fallback policies also use it for retryable 4xx, 5xx, and network failures and fallback switching) [default: 5]
      --dry-run                    Print the workflow flow without executing it
      --cleanup-after-pr           Delete local worktree and branch after PR creation
      --no-cleanup-after-pr        Keep local worktree and branch after PR creation
      -f, --flag <NAME[,NAME...]> Enable named workflow flags; NAME=on|off|true|false also works (repeatable; conflicts with --all)
```

**ワークフラグ。** workflow config では `flags:` を宣言し、`flag: <name>` でステップに関連付けられます。有効値が on の場合だけ flag 付きステップを実行します。未指定の flag はオフです。`--flag NAME`、`--flag e2e,formal_methods` のような複数指定、または `--flag` の繰り返しで有効化します。`-f` は `--flag` の alias です。明示的な `NAME=on|off|true|false` も使えるため、`--flag NAME=off` で保存済みの有効値を上書きできます。すべての `cruise run` は flag selector を出さず、保存済み flag 値と CLI で明示した override を統合します。新規 run、resume、`run --all`、`cruise list` からの Run にも適用されます。未知の名前は拒否され、`--flag` と `--all` は併用できません。`-f/--flag` が指定されていない場合、Approve と Execute now では skip 選択の前に flag を選びます。selector では保存済みの有効 flag が事前選択され、Space で切り替え、Enter で確定します。何も選ばなければ宣言済み flag はすべてオフです。

`--all` は Planned または Suspended の全 session を既定で順次実行します。session を最初に作った時点の mode にかかわらず、worktree mode が強制されます。全 session 終了後に各結果と PR link を示す summary table が表示されます。`--all` と `[SESSION]` は併用できません。

`--parallelism <N>` は invocation 1 回に限る override で、`--all` 中に最大 `N` session を並行実行します。省略時は `1`（順次）、最小値も `1` です。`--all` なしでは拒否されます。各 session は個別の worktree で実行され、ある session の失敗で他 worker が停止することはありません。Ctrl+C は実行中 session を suspend し、新たなスケジュールを停止します。

`--auto-requeue-max-attempts <N>` は今回の batch だけに適用する自動再キュー上限の override です。既定値は `0`（無効）です。

stderr が対話型 terminal で `--dry-run` がない場合、`run --all` は各スケジュール済み session のタイトル、現在ステップ、状態、経過時間を示すライブ dashboard を表示します。CI などの非 TTY 環境や `--dry-run` では通常の log 出力と最後の summary を使います。

<a id="cruise-exec"></a>
## `cruise exec`

```
cruise exec [OPTIONS] [INPUT]

Arguments:
  [INPUT]  Task description bound to {input} (optional if your config doesn't reference {input})

Options:
  -c, --config <PATH>              Path to the workflow config file; use __builtin__ or builtin:<name> for a built-in workflow (see Config File Resolution)
      --instant                    Build a single-agent workflow without a workflow YAML (requires INPUT; conflicts with --config and --flag)
      --sdk <NAME>                 Instant SDK backend: jcode only (requires --instant)
      --model <MODEL>              Instant model (requires --instant)
      --review                     Add a bounded review loop (requires --instant)
      --review-rounds <N>          Maximum review passes, at least 1 (requires --review; default: 2)
      --verify <COMMAND>           Add a verification command (requires --instant; repeatable)
      --max-retries <N>            Maximum number of times a budgeted graph transition may be traversed (no flag default; falls back to the workflow config's top-level `max_retries`, else 3)
      --rate-limit-retries <N>     Maximum number of retries per step (SDK fallback policies also use it for retryable 4xx, 5xx, and network failures and fallback switching) [default: 5]
      --dry-run                    Print the workflow flow without executing it
  -f, --flag <NAME[,NAME...]>     Enable named workflow flags; NAME=on|off|true|false also works (repeatable; see Workflow flags)
```

workflow のステップを現在の directory で直接実行します。plan の生成、git worktree の作成、自動 PR 作成はありません。入力待ちで pause した（`Running`）session と Ctrl+C で中断した（`Suspended`）session は保持され、`cruise run <id>` で再開できます。exec session は `cruise run` の自動選択と `cruise run --all` の対象外です。既存の未 commit 変更は許容され、stash、commit、reset せずその上で実行します。workflow が生成した file と既存変更が混在し得ることを警告します。branch が checkout 済みである必要はあります。

未処理の非ゼロ終了があっても後続のステップは実行されますが、未解決の失敗が残ればコマンドは非ゼロで終了します。失敗したセッションの ID、チェックポイント、ログ、成果物は、再開に成功するか `cruise list` で **Delete** を選ぶまで保持されます。`--instant` と `force_exec` も同様です。原因を解消したら、`cruise list` の **Resume** または `cruise run <id>` で再開してください。再開には元の作業ディレクトリとブランチを使います。`cruise run <id> --dry-run` で再開位置を確認できます。

再開は最初の未解決の失敗ステップから始まります。それ以前に未解決の失敗がないハードエラーの場合は、保存済みのチェックポイントから再開します。再開位置以降のステップは再実行されることがあります。作業ファイルや外部への副作用は元に戻りません。開始済みのワークフローでチェックポイントが欠落、破損、または互換性がない場合、最初からやり直すのではなくエラーになります。

再開時は、`cruise run <id> --max-retries <N>` で上書きしない限り、最初に明示した `--max-retries` が使われます。どちらも指定しない場合はワークフローの既定値が使われます。`--rate-limit-retries` は実行ごとに適用されます。

`--instant` は workflow YAML を解決せず、単一の implement step を持つ workflow をその場で組み立てて使います。指定しない場合、`cruise exec` は通常の設定解決を維持します。config file がない対話型 terminal では分類済み selector に組み込み workflow が表示され、非対話実行では `tdd` を自動選択します。instant には空白でない task が必要です。workflow 既定値で command の pre-step を先頭に追加できます。workflow 既定値と環境 override は引き続き適用され、`CRUISE_SDK`（受け付ける値は `jcode` のみ）と `CRUISE_MODEL` は `--sdk` / `--model` より優先されます。

```sh
cruise exec --instant --sdk jcode --model my-model --review --review-rounds 2 \
  --verify 'cargo test' --verify 'cargo clippy' "implement the feature"
cruise exec --instant --dry-run "implement the feature"
```

Review は確認済みの欠陥を修正し、review pass で file が変更された場合、指定回数（既定 2 回）まで繰り返します。変更がない pass で loop は終わります。検証コマンドは実装と review の後、指定順に実行されます。コマンドが失敗すると run は失敗し、自動修復 loop には入りません。`--max-retries`、`--rate-limit-retries`、`--dry-run` も適用され、`--max-retries` 予算が review 上限より小さい場合は無効です。Instant は workflow flag を宣言せず、`-c` または `-f` と併用できません。branch が checkout 済みの git repository が必要です。リモートでは `cruise ssh <host> -- exec --instant "task"` を使えます。

workflow config に `force_exec: true` がある場合、従来形式の `cruise "task"`、`cruise plan "task"`、`cruise --plan "task"` は、計画、worktree、PR 作成を行わず現在の directory で実行します。`--no-force-exec`、`--repo`、`--grill`、issue input、image attachment は opt-out になりますが、`--skip-planning` と `--no-interactive-planning` は opt-out になりません。

WebUI では Directory ソースに **Exec** ボタンがあり、`force_exec` workflow では **Create & plan** が **Execute** に置き換わります。どちらも指定された既存の絶対パスの作業 directory で直接実行します。CLI の opt-out とは異なり、WebUI の exec は画像添付と `#N` issue 入力を拒否し、暗黙に計画へ切り替えません。これらを使う場合や `force_exec` workflow で計画する場合は **Create draft** を使ってください。Repository ソースでは exec を実行しません。

<a id="cruise---plan"></a>
## `cruise --plan`

```
cruise --plan <INPUT|stdin> [--skip-planning] [--no-force-exec] [--repo <OWNER/REPO>] [--image <PATH>]... [--flag <VALUE>]...
```

session を直ちに作成し、daemon 内で plan 生成を始め、新しい session ID を返します。計画中は `cruise list` に `Planning` と表示されます。生成に失敗すると `Plan Failed` と表示され、計画が成功するまで承認できません。

`--skip-planning` は plan 生成を行わず、入力を直接 `plan.md` に書き込んで、すでに `Planned` の session を作成します。承認ステップは不要です（`force_exec: true` の場合、issue input または別の opt-out がなければ代わりに workflow を直接実行します。明示的に opt-out するには `--no-force-exec` を指定します）。`--plan` なしでも使えます（例: `cruise --skip-planning "task"`）。この場合は `cruise plan --skip-planning "task"` と同じ動作です。

ここでも `--repo <owner>/<repository>` を受け付け、[`cruise plan`](#cruise-plan) と同じように repository を session の対象にします。この経路では `--grill` は利用できません。バックグラウンド計画には interview を行う対話相手がいないためです。

subcommand、`--plan`、位置入力を指定せずに起動した場合、非 TTY stdin は最後まで読み込まれ、内容が位置入力になります。つまり `echo "task" | cruise` は従来の位置引数経路で `cruise plan "task"` のように計画します（`--skip-planning`、`--no-force-exec`、`--repo`、`--image` は引き続き使えます）。TTY で引数なしの起動をすると `cruise list` が開きます。pipe 入力を daemon 内のバックグラウンド計画経路で明示的に読むには `--plan stdin` を渡してください。

<a id="cruise-tell"></a>
## `cruise tell`

```sh
cruise tell <session-id> "Also cover the timeout case"
printf '%s\n' 'Check the logs' | cruise tell <session-id> -
cruise tell <session-id> --log
cruise ssh devbox --cwd /srv/project -- tell <session-id> "Check the logs"
```

生存中で Running フェーズの session にのみ送信できます。指示を省略すると非対話 stdin から読み込みます。`--log` は指示履歴を読み、送信しません。受理されたことは配送の保証ではありません。実行中の agent step は新しい指示を参照できません。次の agent prompt（並列 block 内の prompt child をすべて含む）には追加されますが、command または option step には追加されません。permission、commit policy、出力形式、workflow 遷移は変更できません。

指示は trim され、1 件につき 8,192 bytes 以下です。pending 指示は最大 20 件、合計 65,536 bytes です。run が完了すると未配送指示は期限切れになります。失敗または suspend した run は次回実行用に保持しますが、planned への reset では期限切れになります。指示に secret を含めないでください。

<a id="cruise-list"></a>
## `cruise list`

```
cruise list [OPTIONS]

Options:
      --json   Print all sessions as a JSON array to stdout instead of opening the interactive selector
```

対話型ターミナルでは、セッションがない場合でも一覧メニューに **新規セッション**、**クリーンアップ**、**一括実行**、**終了** が表示されます。セッションを選ぶと、そのフェーズに応じた操作メニューが開きます。作成方法、確認、操作キーについては [`cruise list` のアクション](sessions.md#cruise-list-actions)を参照してください。`--json` を指定すると、確認を求めずに全セッションを JSON 配列として出力して終了します。

<a id="cruise-diff"></a>
## `cruise diff`

```
cruise diff <SESSION_ID> [--stat]
```

session branch の patch を確認するか、`--stat` で概要を表示します。worktree があれば、commit 済み／未 commit の tracked 変更と untracked file を含みます。worktree がない場合は commit 済み branch 変更だけが表示されます。

terminal があり `less -R` が使える場合、完全な patch は `less -R` で表示されます。それ以外では stdout に直接出力します。`--stat` も stdout に直接出力します。

この機能には worktree branch を持つ session が必要です。Current-branch session は未対応です。PR 作成後の cleanup で branch が削除される場合があります（既定の `builtin:tdd` workflow も含む）。cleanup 後は **Open PR** から変更を確認してください。diff 出力はテキストであり JSON ではありません。

<a id="cruise-config"></a>
## `cruise config`

```
cruise config [OPTIONS]

Options:
      --set-parallelism <N>
          Set the maximum number of sessions the WebUI runs concurrently in `run --all` mode.

          Must be >= 1. Omit to show the current configuration. The CLI's one-shot `run --all --parallelism` override is independent.
      --set-language <LANG|auto>
          Set the UI language, or clear the saved setting with auto.
      --set-auto-requeue-max-attempts <N>
          Set the automatic retry limit for Run All (0 disables it).
```

`$XDG_CONFIG_HOME/cruise/`（既定: `~/.config/cruise/`）に保存された `config.json` または `config.yaml` のアプリケーション設定を表示または更新します。workflow ごとの YAML 設定とは別のものです。flag なしでは現在の設定を出力します。`--set-parallelism <N>` は `run_all_parallelism`（既定 `1`）を設定し、**WebUI** の `run --all` で並行実行する session 数を制御します。CLI はこの保存値を読み込みません。1 回限りの値には `cruise run --all --parallelism <N>` を使ってください。

`cruise config --set-language ja` はアプリケーション設定 `uiLanguage` を保存します（読み込みでは `ui_language` も受け付けます）。`--set-language auto` は保存済みの言語を消去し、自動選択に戻します。明示的に指定できる tag は `en`、`zh-CN`、`es`、`fr`、`pt-BR`、`ja`、`ko` です。大文字小文字と、hyphen の代わりの underscore は許容されますが、`pt` や `en-US` などは受け付けません。`cruise config` は実効 `ui_language` とその適用元（`env`、`config`、`system`、`default`）を表示します。

CLI の UI 言語は `CRUISE_UI_LANGUAGE` > アプリケーション `uiLanguage` > system locale > `en` の優先順です。空または `auto` の環境値、未設定または `auto` の config 値は、その優先層を未指定として扱います。無効な環境値は英語で警告して無視します。CLI setter とアプリケーション config の検証では無効値を拒否します。

アプリケーション設定でもグローバル出力設定を指定できます。対応 key と優先順位は[グローバル出力設定](configuration.md#output-preferences)を参照してください。これらの設定は UI 言語を変更しません。

`cruise config` は設定済みの値を JSON value line で表示します（例: `languages = {"plan":"Japanese","pr":"English"}`、`commit_message_template = "{title} [{session_id}]"`、`pr_body_template = "## Summary\n{body}"`）。他のアプリケーション設定を更新しても、これらの値は保持されます。後から `default.yaml` やアプリケーション設定を編集しても、保存済み snapshot は変わりません。対応する workflow override 環境変数を明示的に送った CLI request では、その request の間だけメモリ上で override されますが、snapshot は書き換えません。

`cruise config --set-auto-requeue-max-attempts <N>` は CLI と WebUI の Run All 用に `run_all_auto_requeue_max_attempts` を設定します。CLI の `--auto-requeue-max-attempts <N>` flag は 1 batch だけ上書きします。

macOS で実行中にスリープさせないには、machine-level `config.yaml` に `prevent_sleep: true`（または `config.json` に `"prevent_sleep": true`）を設定します。既定値は `false` です。CLI と WebUI の run が対象ですが、計画だけの場合は対象外です。その他の OS では何もしません。変更は次の run から有効です。プロジェクト `default.yaml` ではこの設定をサポートしません。

両方の形式が存在する場合は `config.json` が優先され、それがなければ既存の `config.yaml` が使われます。2 つのファイルは統合されません。更新は選択された形式で書き戻されます。YAML comment は維持されません。

<a id="notifications"></a>
### 通知

workflow YAML ではなくアプリケーション設定ファイルに `notifications` を設定します。各 channel に master switch と、任意の event 別 switch を指定できます。

```yaml
notifications:
  desktop: true   # default: true
  sound: true     # default: false
  webhook: true   # default: true, but requires the environment variable below
  events:
    actionRequired: { sound: false }
    planReady: { desktop: false }
    completed: { webhook: false }
    failed: { desktop: true, sound: true, webhook: true }
```

event key は `actionRequired`、`planReady`、`completed`、`failed` です。`action_required` と `plan_ready` も受け付けます。各 event に `desktop`、`sound`、`webhook` を指定できます。event の switch を省略すると true ですが、無効な channel master switch を上書きすることはできません。sound は desktop banner とは独立しています。Windows では desktop banner と sound は送信しませんが webhook は使えます。

Cruise のプロセス環境に Slack Incoming Webhook URL を `CRUISE_NOTIFY_WEBHOOK` として設定します。secret URL を設定ファイルに書かないでください。未設定、空、または無効な HTTP(S) URL では webhook 配信が無効になります。送信先には機微な task detail が届く可能性があるものとして扱ってください。

配信はベストエフォートです。既定以外の通知設定も `cruise config` に表示されますが、webhook URL は表示されません。

ワークフローイベント（**Action required**、**Plan ready**、**Completed**、**Failed**）の通知は、CLI と WebUI のどちらが作業を開始しても daemon が送信します。サウンドと webhook は常に daemon から配信されます。WebUI browser tab が接続されている場合、デスクトップ通知は browser notification として表示されます。接続された WebUI browser がない場合、daemon がネイティブのデスクトップ通知を表示します（macOS/Linux のデスクトップセッション）。`desktop` master と event switch は daemon の通知とブラウザー通知の両方を制御しますが、ページ内 toast は制御しません。daemon の環境に `CRUISE_DISABLE_NOTIFICATIONS=1` を設定し、`cruise daemon restart` で再起動すると、daemon のデスクトップ通知、サウンド、webhook 配信が無効になります。WebUI のブラウザー通知は無効になりません。これ以外の値ではこの全体無効化 switch は有効になりません。

両形式の `model` と `plan_model` では、任意のグローバル model 既定値を設定できます。それぞれ文字列または順序付き配列を受け付け、先頭項目が primary、残りは SDK fallback です。2 形式で同じ field 名と alias を使います。`config.yaml` の例:

```yaml
runAllParallelism: 3
model:
  - openai/gpt-6-luna:max
  - openai/gpt-6.1-sol:xhigh
plan_model:
  - openai/gpt-6.1-sol:xhigh
  - openai/gpt-6-luna:max
```

workflow と `default.yaml` の model 設定はこれらの既定値より優先され、環境 override はさらに優先されます。どの層にも `plan_model` がない場合、計画では実効 `model` を使います。SDK 配列は既存の[フォールバック動作](sdk.md#sdk-mode)を使い、command mode では primary だけを使います。`planModel` は `plan_model` の alias として受け付けます。parallelism が未設定なら `1` が既定となるため、model のみの設定も有効です。CLI と WebUI で parallelism を更新しても両 model 設定は保持されます。

同じアプリケーション設定ファイルで、対話型 workflow selector のカテゴリを構成できます。

```yaml
workflow_categories:
  Development:
    workflows:
      - builtin:tdd
      - refactor: Refactor an existing codebase
    Backend:
      workflows: [api]
show_others_category: true
others_category_name: Others
```

`workflow_categories` は順序を持つ category map の tree です。各 category は `workflows` list と入れ子の category を含められます。項目には名前、または selector の説明を上書きする値を持つ単一 key map を指定します。`show_others_category` の既定値は `true`、`others_category_name` は `Others` です。これらは application `config.yaml` または `config.json` に置き、workflow file や `default.yaml` には置かないでください。照合と fallback の動作は[設定ファイルの解決](configuration.md#config-file-resolution)を参照してください。

workflow と application settings は `model` / `plan_model` に文字列と `{model, disabled_during?}` object を含む配列を指定できます。

```yaml
model:
  - openai/gpt-6.1-sol:medium
  - model: claude/claude-opus-4-6:high
    disabled_during: ["mon-fri 01:00-04:00"]
  - openrouter/z-ai/glm-5.3-flash:high
```

JSON でも同じ文字列配列と `{"model":"...","disabled_during":[...]}` object を使います。文字列は workflow の SDK を継承し、省略時は `jcode` です。`openai/` や `openrouter/` は SDK 名ではなく model provider です。object の `sdk` field は使用できず、`SDK-qualified model objects are unsupported; use a plain model string` という validation error になります。object は空でない `model` を必要とし、未知の key を拒否します。object と `command` は併用できません。文字列だけの command array は先頭 model のみ使います。環境 model override は配列全体を置き換えます。computer use には jcode が必要です。`mcp_servers` は stdio のみ対応し、HTTP/SSE entry は validation error です。

<a id="candidate-unavailable-windows-disabled_during"></a>
## 候補を利用できない時間帯（`disabled_during`）

`model` または `plan_model` 配列内で、候補ごとの利用不可時間帯を宣言できます。workflow、レイヤー化された `default.yaml`、application JSON/YAML 設定（`planModel` alias を含む）で利用できます。これはユーザーが管理する除外時間であり、価格連動 routing や provider preset ではありません。

```yaml
model:
  - model: openai/gpt-5.5
    disabled_during: ["mon-fri 01:00-04:00", "mon-fri 06:00-10:00"]
  - openai/gpt-5.4
```

同じ時間帯を固定 `+09:00` offset で指定する例:

```yaml
model:
  - model: openai/gpt-5.5
    disabled_during: ["mon-fri 10:00-13:00 +09:00", "mon-fri 15:00-19:00 +09:00"]
  - openai/gpt-5.4
```

時間帯の文法は `[days ]HH:MM-HH:MM[ offset]` です。曜日は大文字小文字を区別しない `mon`〜`sun`、カンマ区切り（`sat,sun`）、または末尾から先頭へ折り返す包含範囲（`mon,wed-fri`、`fri-mon`）です。曜日を省略すると毎日になります。時刻は時と分を 2 桁で指定します。開始は `00:00`〜`23:59`、終了は `00:01`〜`24:00` です。開始と終了が同じ指定は無効です。終日には `00:00-24:00` を使ってください。開始時刻を含み、終了時刻は含みません。終了時刻が開始時刻より前なら午前 0 時をまたぎ、曜日は **開始日** を示します。`fri 22:00-02:00` は土曜 01:00 を含みますが、日曜 01:00 は含みません。

offset の既定値は UTC です。`UTC`、`Z`、最大絶対値 `14:00` の固定 `+HH:MM` / `-HH:MM` offset を受け付けます。IANA 名、夏時間の切り替え、ローカル system time zone は未対応です。空 list、空の時間帯、不正な時間帯は検証エラーになります。

Cruise は各 prompt の開始時と retry 中の候補切り替え時に時間帯を確認します。streaming 中の turn を途中で中断することはありません。一致する時間帯があれば、retry policy／retry 予算とは独立して候補を除外します。全候補が無効時間帯または cooldown 中なら backend を起動せず prompt は失敗します。

無効な候補も capability 検証の対象です。override は base model が primary 候補と一致する場合に限り、workflow の配列と時間帯を継承します。それ以外の override は workflow list を継承しません。環境 model override は時間帯も含め、配列全体を置き換えます。

<a id="cruise-clean"></a>
## `cruise clean`

```
cruise clean
```

各 Completed session の PR status を `gh pr view`（GitLab では `glab mr view`）で確認します。PR が close または merge 済みの session は worktree（残っている `--repo` clone も含む）とともに削除します。開始されておらず `plan.md` が欠落または空白だけの Planned current-branch session も削除されます。Suspended session とその他の planned session は再開用に保持されます。

成功した exec セッションは自動的に削除されます。`cruise clean` は失敗した exec セッションを削除しません。作業ファイルを変更せずにセッションを破棄するには、`cruise list` で **Delete** を選びます。

> **注意:** `gh pr create` が失敗、または実行に至らなかった場合、session に PR URL がないことがあります。PR に紐付く URL なし session は保持されます。session log を確認するか `gh pr create` を手動で再実行してください。

<a id="cruise-usage"></a>
## `cruise usage`

`cruise usage [--by session,step,purpose,backend,model] [--session ID] [--format text|json|csv]` はローカルの試行記録を要約します。既定の grouping は `step,backend,model`、出力形式は text です。provider 値が欠落している場合は 0 ではなく unknown です。合計には input と output だけが含まれます。コストは provider の報告値であり推定値ではありません。
