[English](../en/configuration.md) · [日本語](../ja/configuration.md) · [简体中文](../zh/configuration.md) · [目次](README.md)

# 設定

<a id="config-file-resolution"></a>
## ワークフロー設定ファイルの解決

cruise は次の順序でワークフロー設定を解決します。

1. **`-c/--config` フラグ** — 最優先です。指定したファイルが存在しない場合、cruise はエラーで終了します。選択プロンプトは表示されません。`-c __builtin__` は組み込みの `tdd` ワークフローを選び、`-c builtin:tdd` と同じです。`-c builtin:<name>` はカタログ内の名前付きワークフローを選びます。旧セレクター `builtin:default`、`builtin:simple`、`builtin:review` はサポートされません。固定カタログは次のとおりです。

   | 名前 | 説明 |
   |------|-------------|
   | `tdd` | テスト先行の実装に続いて、並列の analyze → consolidate → apply 検証グループと PR ワークフローを実行 |
   | `refactor` | 差分を起点にコード、テスト、構造を改善し、検証 |
   | `add-tests` | 動作とユーザーフローの網羅状況を監査し、有意義なテストと CI を追加してから独立して検証し、不足を修正 |
   | `add-formal-specs` | Quint、Alloy、Gherkin の実行可能な仕様と CI を追加し、実際の仕様・ソースの欠陥を修正 |
   | `research` | 根拠ソースに基づく限定的な読み取り専用調査、独立レビュー、出典付きレポート |
   | `security` | 範囲を定めたセキュリティ棚卸しと並列監査、トリアージ、確認済み欠陥の安全な修正、独立検証 |
   | `pr-review-fix` | 既存の GitHub PR ヘッドブランチ上で、対応可能なレビューコメントに対処 |

   `refactor`、`add-tests`、`add-formal-specs`、`security` は既定で `force_exec: true` です。ローカルの計画を外側の plan/session、worktree、自動 PR なしで現在のディレクトリ内で直接実行します。通常の plan/session 経路を使うには `--no-force-exec` を指定してください。

   `cruise workflow list` はカタログを表示します。組み込みワークフローをカスタマイズするには `cruise workflow eject <name> [--to user|project]` を使います。例: `cruise workflow eject tdd --to project`。eject が受け付けるのはカタログ名のみで、既存ファイルを上書きしません。

   `refactor` は、リポジトリに既にあるステージ済み・未ステージ・未追跡の変更に加え、信頼できる base/merge-base が利用できる場合はタスクブランチとの差分を起点にします。変更範囲はその差分と必要な呼び出し元・依存関係に限定されます。差分がない場合はリポジトリ全体が対象です。既存の作業は保持され、無関係な編集を reset、stash、revert しません。

   `research` workflow と `security` の inventory/audit/triage フェーズには読み取り専用の SDK permission が必要です。workflow と適用される user/project default の解決後に `command:` も `sdk:` も残らない場合、Cruise は既定の `sdk: jcode` を使います。`command:` backend は読み取り専用に制限できず、制限付き permission step では拒否されます。

   対話 selector（CLI、TUI、WebUI）は組み込みやその他の候補をカテゴリ別に表示します。設定ファイルがない非対話解決では `tdd` が使われます。

   `create_pr` は workflow 専用の boolean で、既定値は `true` です。`false` にすると通常の session、artifact、worktree は保持したまま、Cruise による自動 commit、push、PR 作成、`after-pr`、PR ベースの cleanup を省略します。`create_pr` は user/project `default.yaml` では拒否され、CLI option ではありません。

2. **`CRUISE_CONFIG` 環境変数** — 設定されていればその値を直接使います（ファイルが存在しない場合はエラー）。選択プロンプトは表示されません。
3. それ以外では cruise は以下の場所からすべての候補を収集し、選択肢として提示します。
   - 現在のディレクトリ: `./cruise.yaml` → `./cruise.yml` → `./.cruise.yaml` → `./.cruise.yml`
   - 現在のディレクトリの `./.cruise/*.yaml` / `*.yml`（ファイル名順。最上位の `./.cruise/default.yaml` は除外）
   - `$XDG_CONFIG_HOME/cruise/workflows/*.yaml` / `*.yml` および有効なロック済み bundle directory のメンバー（既定: `~/.config/cruise/workflows/`）。相対パス順。

   stdin と stdout の両方が TTY のとき、候補と組み込み workflow をカテゴリ付きの対話 selector で提示します。pipe 接続 stdin や script などの非対話環境ではプロンプトなしで最優先候補を選びます。
4. **設定ファイルがない場合** — 対話端末では selector に組み込み workflow を提示し、非対話解決ではソースツリーの `builtin/tdd.yaml` にある埋め込み `tdd` workflow を自動選択します。設定ファイルは必須ではありません。

対話 selector は設定された `workflow_categories` ツリーを使います。未設定または空なら、既定の Development (`tdd`、`refactor`)、Quality (`add-tests`、`add-formal-specs`)、Research (`research`)、Security (`security`) を使います。空でない user ツリーは既定値にマージせず置き換えます。カテゴリと workflow 項目は宣言順を保ちます。CLI サブメニューではサブカテゴリを workflow 項目より先に表示します。workflow は複数カテゴリに掲載できます。

未分類の user と組み込み workflow は `Others` または設定された `others_category_name` に表示されます。`show_others_category: false` にすると非表示になります。カテゴリは対話表示にだけ影響し、明示選択や非対話解決の優先順位は変えません。

<a id="workflow-defaults-defaultyaml"></a>
### ワークフロー既定値 (`default.yaml`)

`$XDG_CONFIG_HOME/cruise/default.yaml` は user default、`./.cruise/default.yaml` は project default を設定します。`XDG_CONFIG_HOME` 未設定時の既定 path は macOS/Linux で `~/.config/cruise/default.yaml`、Windows では `%APPDATA%\cruise\default.yaml`（`APPDATA` がない場合は `~/.config/cruise/default.yaml`）です。これらのファイルは workflow が未指定の field に既定値を与えます。workflow として選択することはできず、`-c` または `CRUISE_CONFIG` で指定するとエラーになります。

該当する項目での優先順位（高い順）: invocation ごとの CLI flag（workflow 選択用 `-c` を含む） > 設定環境変数による override > workflow file > `./.cruise/default.yaml` > `$XDG_CONFIG_HOME/cruise/default.yaml` > アプリケーション model default (`config.json` または `config.yaml`) > 組み込み default。通常の prompt では step-level model override が引き続き優先されます。

実際の jcode prompt に有効な model がない場合、Cruise は `$XDG_CONFIG_HOME/cruise/default.yaml`（またはプラットフォーム標準の user config path）に `model: null` を持つ template を作成し、jcode を起動する前に停止します。user-level file を編集して実際の `provider/model[:effort]` reference または bare model を設定してください。新しい session では `config.json`、`config.yaml`、workflow、`CRUISE_MODEL` で `model` を設定することもできます。planning call では `plan_model` / `CRUISE_PLAN_MODEL` を使えます。

設定後は、最初に処理を始めたコマンドを再試行してください。失敗した run session には `cruise run <SESSION>`、plan や exec の要求にはそれぞれ `cruise plan` または `cruise exec` を再実行します。file-backed session は default を再読み込みしますが、snapshot session は後からの default や環境変更を再適用しません。更新後の設定を使うには新しい snapshot session を開始してください。

- `env` と `mcp_servers` は key ごとにマージし、上位レイヤーが優先されます。
- `command` と `sdk` は一組です。あるレイヤーがどちらかを設定すると、下位レイヤーの組は無視されます。
- その他の field (`model`、`plan_model`、`subagent_model`、`max_retries`、`interactive_planning`、`languages`、`cleanup_after_pr`、`force_exec`、`computer_use`、`permission`、`disallowed_tools`、`retry`、`formal_verify`) は、最上位で設定されたレイヤーの値を使います。制限リストはレイヤー間で置換されます。workflow の `disallowed_tools: []` を明示すると継承した default をクリアします。
- Worktree 設定 `base_branch` と `auto_fetch` は default 専用 key で、project 値が user 値に優先します。マージ後の `base_branch` が空であれば `auto_fetch: true` は無効です。
- default が workflow と異なる種類の backend（`command` / `sdk`）を指定し、backend 固有の未設定 field（`model`、`retry`、`mcp_servers` など）を補う場合は検証エラーになります。workflow で field を設定するか、`default.yaml` から削除してください。

`pre_steps:` は command 専用 step (`command`、`env`、`timeout`、`skip`、`when`) を定義し、workflow の `steps` の先頭に `pre/<name>` として挿入します。planning や `after-pr` では実行されません。user step の後に project step を並べ、同名 project step は user step をその場で置き換えます。

```yaml
# ./.cruise/default.yaml
pre_steps:
  mise-trust:
    command: mise trust || true
  nix:
    command: nix develop --command true
```

<a id="config-file-reference"></a>
## 設定ファイルのリファレンス

<a id="basic-structure"></a>
### 基本構造

`formal_verify.model_check_timeout_seconds` は、Quint model-check と Alloy の各コマンドの timeout を秒単位で指定します。1 から 86400 までの整数を受け付け、既定値は 900 です。workflow YAML またはレイヤー化された `default.yaml` に設定できます。

```yaml
formal_verify:
  model_check_timeout_seconds: 900
```

```yaml
command:                   # LLM invocation command (mutually exclusive with `sdk`)
  - claude
  - --model
  - "{model}"
  - -p

# sdk: jcode              # Use jcode-sdk (the fallback only when neither
                          # `command` nor `sdk` remains set after applicable
                          # `default.yaml` layers; see SDK Mode)
# sdk: claude             # Use claude-agent-sdk to drive the claude CLI

description: |             # one-line summary shown next to the filename in selectors (optional)
  Team-shared review-heavy flow with auto-PR.

model: sonnet             # default model for all prompt steps (optional)
# In SDK mode, arrays use the first model, then fall back in order.
# model: [sonnet, opus, gpt-5.5]
plan_model: opus          # model used for the built-in plan step (optional)
# subagent_model: haiku   # SDK-only: one subagent model, string or {sdk: claude, model: haiku}
languages:                # prompt languages (optional; defaults to English)
  pr: English             # language for auto-generated PR title/body
  plan: English           # language used by built-in planning prompts
# commit_message_template: "{title} [{session_id}]" # host-created commit message
# pr_body_template: "## Summary\n{body}"            # generated PR body wrapper
# force_exec: false       # execute direct plan entry points in place (use --no-force-exec to opt out)
# permission: full        # prompt permission: read-only | edit | full (default full)
# disallowed_tools: [web, subagent, "shell:npm publish"] # SDK prompt deny list
# computer_use: false     # jcode only, macOS: let prompts control the desktop (macos_computer_use)
# Deprecated compatibility fields: pr_language and plan_language

env:                      # environment variables applied to all steps (optional)
  API_KEY: sk-...
  PROJECT: myproject

# MCP servers are available to SDK runs only; omit this block for command mode.
# mcp_servers:
#   local_search:
#     command: npx
#     args: ["-y", "@example/search"]

flags:                    # workflow flags (optional; absent values default off, see Workflow flags)
  # run_review: {}

groups:                   # step group definitions (optional)
  review:
    if:
      file-changed: test
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify
      coderabbit:
        prompt: /cr

steps:
  step_name:
    # step configuration
    # flag: run_review     # run only when the declared flag is on (optional)

after-pr:                # optional: steps that run automatically after PR creation
  step_name:
    # step configuration (same format as `steps`)
```

未知 key の扱いは階層によって異なります。個々の step map (`steps`、`after-pr`、group の `steps`、parallel child) と `retry` block は未知 field を parse error として拒否します。typo や削除済み field `fail-if-no-file-changes` などは設定エラーになります。一方、未知の **top-level** key は黙って無視されるため、ファイル最上位で `plan_model` や `max_retries` の名前を間違えても受理されますが、効果はありません。

<a id="dynamic-model-selection"></a>
### model の動的選択

`command` 配列に `{model}` placeholder がある場合、cruise は各 step の有効 model に応じて実行時に解決します。

- **model を指定した場合**（top-level `model` または step-level `model`）: `{model}` を model 名に置換します。
- **model を指定しない場合**: `{model}` 引数と、その直前の `--model` flag を自動削除します。

step-level `model` field は、その step に限り top-level `model` default を上書きします。

テンプレート変数と同じ波括弧 escape（[変数リファレンス](steps.md#variable-reference)参照）が使われます。`{{model}}` は placeholder ではなく文字列 `{model}` です。それ以外の未 escape `{name}`、空の `{}`、閉じていない `{`、単独の `}` はテンプレート構文エラーです。

```yaml
command:
  - claude
  - --model
  - "{model}"      # replaced at runtime, or --model/{model} pair is stripped if no model
  - -p

model: sonnet      # default; steps without model: use this

steps:
  planning:
    model: opus    # overrides the default for this step only
    prompt: "Create a plan for: {input}"
```

<a id="prompt-languages"></a>
### prompt の言語

UI の表示言語は prompt の言語と独立しています。アプリケーション設定の `uiLanguage`、`CRUISE_UI_LANGUAGE`、または TUI の言語選択を使って設定します。WebUI では、ブラウザー側の言語選択で server default を上書きできます。これらの設定は `languages.pr` や `languages.plan` を変更しません。

`languages.pr` は自動生成する PR title/body の言語、`languages.plan` は cruise 組み込み planning prompt の言語を指定します。`CRUISE_LANGUAGE_PR` と `CRUISE_LANGUAGE_PLAN` はそれぞれ対応する YAML 値を上書きし、空白の環境変数値は無視されます。それ以外では `LC_ALL`、`LC_MESSAGES`、`LANG`、`LANGUAGE` のうち最初の空でない値を1回マッピングして使います（未対応値でも次の変数には進みません）。それもなければ `English` です。

```yaml
languages:
  pr: Japanese             # PR title/body will be generated in Japanese
  plan: Japanese           # generated/updated plans and plan answers will be in Japanese
```

有効値は組み込み template で `{pr.language}` と `{plan.language}` として利用できます。

<a id="commit-and-pr-templates"></a>
### commit と PR の template

省略可能な top-level string key `commit_message_template` と `pr_body_template` は、worktree PR 作成経路でホストが作成する commit と PR body をカスタマイズします。workflow、user `<config_dir>/default.yaml`、または project `.cruise/default.yaml` に設定します。優先順位は workflow > project default > user default > 未設定です。CLI option や環境変数による override ではありません。

```yaml
commit_message_template: "{title} [{session_id}]"
pr_body_template: |
  ## Summary
  {body}
```

| Template | 利用できる変数 |
|----------|-------------------|
| `commit_message_template` | `{title}`, `{issue}`, `{session_id}`, `{branch}` |
| `pr_body_template` | `{title}`, `{body}`, `{issue}`, `{session_id}`, `{branch}` |

`{title}` は LLM 生成 PR title、`{body}` は LLM 生成 PR body、`{issue}` は現在常に空文字列、`{session_id}` は session ID、`{branch}` は worktree branch 名です。これらの template は issue を閉じる文言を自動追加しません。

未知の変数と不正な構文は workflow の load/compile 時に拒否されます。文字どおりの波括弧は `{{` と `}}` で escape します。

両 template が未設定なら従来の動作は変わりません。commit は生成済み PR title または既存の input fallback を使い、PR title が空の場合は `--fill` を使います。これらの設定は current-branch run、`create_pr: false`、agent が作成する commit、squash-merge message には影響しません。

<a id="session-title-generation"></a>
### session title の生成

plan の承認後、cruise は raw task input の代わりに `cruise list` と WebUI sidebar に表示する、最大80文字の簡潔な session title を生成します。

<a id="environment-variables"></a>
### 環境変数

`CRUISE_JCODE_CLI_PATH`、`CRUISE_CLAUDE_CLI_PATH`、`CRUISE_OPENCODE_CLI_PATH` は Cruise process 環境から provider executable を選択します。workflow の設定 key ではなく、workflow `env:` からは指定できません。

環境変数は workflow と step の両方で設定できます。step-level 値は、その step についてだけ workflow-level 値を上書きします。parallel child には第3のレベルが加わり、workflow < parallel block < child の優先順位になります。値は template 変数置換に対応します。各 child は block より前の変数値を基準にそれぞれ解決します。

process-level 設定 override は workflow と default の解決後に適用されます。対応する名前と値の型:

| 変数 | 設定 key | 値 |
|---|---|---|
| `CRUISE_MODEL` | `model` | model |
| `CRUISE_PLAN_MODEL` | `plan_model` | model |
| `CRUISE_SUBAGENT_MODEL` | `subagent_model` | model entry |
| `CRUISE_SDK` | `sdk` | text |
| `CRUISE_COMMAND` | `command` | JSON array |
| `CRUISE_MAX_RETRIES` | `max_retries` | integer |
| `CRUISE_INTERACTIVE_PLANNING` | `interactive_planning` | boolean |
| `CRUISE_CLEANUP_AFTER_PR` | `cleanup_after_pr` | boolean |
| `CRUISE_FORCE_EXEC` | `force_exec` | boolean |
| `CRUISE_COMPUTER_USE` | `computer_use` | boolean |
| `CRUISE_PERMISSION` | `permission` | text: `read-only`、`edit`、`full` |
| `CRUISE_DISALLOWED_TOOLS` | `disallowed_tools` | JSON array |
| `CRUISE_COMMIT_MESSAGE_TEMPLATE` | `commit_message_template` | text |
| `CRUISE_PR_BODY_TEMPLATE` | `pr_body_template` | text |
| `CRUISE_RETRY` | `retry` | JSON object または `null` |
| `CRUISE_RETRY_BASE_DELAY_MS` | `retry.base_delay_ms` | integer |
| `CRUISE_RETRY_MAX_DELAY_MS` | `retry.max_delay_ms` | integer |
| `CRUISE_RETRY_MODEL_FALLBACK` | `retry.model_fallback` | boolean |
| `CRUISE_RETRY_FALLBACK_CHAINS` | `retry.fallback_chains` | JSON object |
| `CRUISE_LANGUAGES` | `languages` | JSON object |
| `CRUISE_LANGUAGE_PR` | `languages.pr` | text |
| `CRUISE_LANGUAGE_PLAN` | `languages.plan` | text |
| `CRUISE_RUN_ALL_PARALLELISM` | app `run_all_parallelism` | integer、1以上 |

通常は key path を大文字にして `CRUISE_` を付け、ドットとハイフンを underscore に置換します。言語の子 key は上記の綴りのみサポートされます。値は trim され、空白だけの値は無視されます。boolean は `true`、`false`、`1`、`0` のみ、integer は符号なし10進数字のみ受け付けます。text はそのままです。model 値は通常、単一 model の文字列です。ただし `[` または `{` で始まる場合は JSON として parse し、その field の通常の model 形状に一致する必要があります。特に `CRUISE_MODEL` と `CRUISE_PLAN_MODEL` は JSON array を受け付けるようになりました。以前は `[` や `{` で始まる値も文字どおりの model 名として扱われていました。fallback entry を持つ array は YAML と同じように model fallback を有効にします。その他の複合値には YAML flow syntax ではなく JSON が必要です。対応する任意値には空欄ではなく JSON `null`、対応する空 list には `[]` を使ってください。不正値は変数名を含むエラーになり、通常の設定検証も行われます。

親 object を子 override より先に適用します。そのため `CRUISE_RETRY_MAX_DELAY_MS` は `CRUISE_RETRY` 内の対応 field を上書きし、`CRUISE_LANGUAGE_PR` は `CRUISE_LANGUAGES` を上書きします。`CRUISE_SDK` を設定すると設定済み `command` が消え、`CRUISE_COMMAND` を設定すると設定済み `sdk` が消えます。両方の変数に空でない値を設定するとエラーです。例:

```sh
export CRUISE_MODEL='["provider/primary", "provider/fallback"]'
export CRUISE_RETRY='{"base_delay_ms":1000}'
export CRUISE_RETRY_MAX_DELAY_MS=5000
```

`CRUISE_RUN_ALL_PARALLELISM` は TUI/WebUI run-all 操作と有効設定表示で app 設定を上書きしますが、設定ファイルには保存されません。`--parallelism` を使う CLI `run --all` は変更しません。

`CRUISE_UI_LANGUAGE` は workflow field や生成 prompt の言語ではなく、process UI の言語を制御します。対応 tag と優先順位は [`cruise config`](cli-reference.md#cruise-config) を参照してください。

`CRUISE_COMMIT_COAUTHOR_NAME` と `CRUISE_COMMIT_COAUTHOR_EMAIL` は、PR 用に cruise が作成する commit に `Co-authored-by:` trailer を加えます。両方とも設定され、空でない必要があります。名前に `<`、`>`、改行が含まれる場合、または address が不正な場合は、commit を失敗させず trailer を無効にします。

`CRUISE_SKIP_AGENT_AUTH_CHECK=1` は SDK 起動時 OAuth 期限切れ検査（SDK mode の説明を参照）を回避します。process 専用の runtime switch であり workflow 設定 override ではありません。正確な値 `1` だけが検査を省略します。

`CRUISE_USAGE_RECORDING` は process 専用 runtime switch です。ローカル usage 記録を無効にするには `0` または `false` を設定します。`1`、`true`、未設定は記録を有効にします。値は trim して大文字小文字を区別せず扱います。それ以外の値では警告を出して記録を有効にします。

```yaml
env:                        # top-level: applied to all steps
  ANTHROPIC_API_KEY: sk-...
  TARGET_ENV: production

steps:
  deploy:
    command: ./deploy.sh
    env:                    # step-level: merged over top-level env
      TARGET_ENV: staging   # overrides top-level value for this step only
      LOG_LEVEL: debug
```

<a id="application-configuration"></a>
## アプリケーション設定

Cruise はアプリケーション設定を workflow YAML とは別に `$XDG_CONFIG_HOME/cruise/config.json` または `config.yaml` に保存します。両方存在する場合は JSON が優先されます。workflow の解決やその他の設定については、上記の[ワークフロー設定ファイルの解決](#config-file-resolution)を参照してください。

<a id="ui-language"></a>
### UI の表示言語

アプリケーション設定の `uiLanguage` を指定するか、`cruise config --set-language ja` を使います。入力時は別名 `ui_language` も受け付けます。明示的に設定できる tag は `en`、`zh-CN`、`es`、`fr`、`pt-BR`、`ja`、`ko` です。大文字小文字の違い、および `_` / `-` の表記揺れも受け付けます。`auto` は未指定を意味し、`cruise config --set-language auto` は保存済み設定を消去します。値を設定またはアプリケーション設定を検証するとき、不正値は拒否されます。

CLI と TUI では、`CRUISE_UI_LANGUAGE` が `uiLanguage` より優先され、その次に system locale、最後に英語となります。空白または `auto` の環境変数値は次の層に委ねます。不正な環境変数値は英語の警告を出して無視します。system locale は `LC_ALL`、`LC_MESSAGES`、`LANG`、`LANGUAGE` のうち最初の空でない値を使います。その値が未対応でも後続の変数は試しません。環境変数から言語を決定できない場合、Windows は user locale も確認します。macOS と Linux は環境変数のみを使うため、環境変数のない GUI 起動では英語が既定になります。

WebUI では、有効な browser cookie `cruise_ui_language` があればそれを使い、なければ server process の既定言語を使います。WebUI の言語選択は cookie を保存してページを再読み込みし、アプリケーション設定は変更しません。UI 言語は、生成 prompt の言語を制御する workflow `languages.pr` と `languages.plan` から独立しています。

<a id="config-hot-reload"></a>
## 設定の hot-reload

`cruise run` 中は、step ごとに設定ファイルの変更を確認します。mtime によって変更が検出されると、再起動なしで更新済み設定を自動再読み込みします。実行中に prompt の調整、step の追加、設定の変更ができます。

> **注意:** hot-reload が適用されるのは、埋め込み組み込み workflow ではなく外部設定ファイルから始めた session のみです。再読み込みを適用するには、現在の step が新しい設定にも存在する必要があります。session に保存済みの値がない新規宣言 flag は off です。

<a id="rate-limit-retry"></a>
## rate limit 時の retry

SDK fallback policy がない場合、command の stderr に rate limit を示す文言または単独の `429` token（`429` を含むだけの別文字列は対象外）があると、cruise は同じ model を指数 backoff で再試行します。SDK backend では `usage limit`、`session limit`、`overloaded` も limit として扱います。

- 初回待機: 2秒
- 最大待機: 60秒
- 既定 retry 回数: 5（`--rate-limit-retries` で変更）

SDK backend は、任意の `retry:` block または fallback entry を含む workflow-level model array を使った fallback policy にも対応します。明示的な `retry:` block で policy を設定できます。

```yaml
retry:
  base_delay_ms: 500        # default: 500
  max_delay_ms: 300000      # default: 300000
  model_fallback: true      # default: true
  fallback_chains:
    default:
      - provider-a/model-a
      - provider-b/model-b
    "provider-a/*":
      - provider-c/*
```

SDK retry policy があると、429・5xx・429 以外の 4xx・ネットワーク障害のとき、待たずに次の fallback model へ切り替えます。使える fallback が無い場合だけ同じ model で再試行します。`retry:` を宣言すると、`model_fallback: false` や空の chain でもこの分類と backoff が有効になります。従来の動作（limit エラーだけを同じ model で再試行）を保つには、`retry:` block を省き、model を scalar にしてください。fallback entry を持つ workflow-level model array は model fallback を有効にします。`command:` backend は `retry:` を無視します。
