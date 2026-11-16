**言語:** [English](../en/github-actions.md) | [日本語](../ja/github-actions.md) | [简体中文](../zh/github-actions.md) · [目次](README.md)

<a id="cruise-github-action"></a>
<a id="github-actions"></a>
# cruise GitHub Action

設定可能な `@cruise` を GitHub Issue でメンションすると、GitHub Actions 内で Cruise が計画と実装を行います。成功した実行で branch commit が残り、自動 PR 作成が有効なら draft PR を開きます。[`anthropics/claude-code-action`](https://github.com/anthropics/claude-code-action) と発想は似ていますが、`sdk: jcode`（解決された workflow が別の backend を選んだ場合を除く）を通じて Cruise 独自の plan → implement → 任意 PR ワークフローを実行します。Action が `jcode` CLI をインストールし、認証情報も設定します。

この Action に **pull request モードはありません**。`issues`（opened）と `issue_comment`（created）イベントにのみ反応します。pull request 上のコメントは常に無視されます（GitHub API 上では PR も「issue」ですが、この Action は明示的に除外します）。

> 注意: 制限付き prompt ステップ（`permission: read-only | edit`）は、対話型の確認ではなくツールを拒否した状態で無人実行されます。read-only ステップが workspace を変更すると失敗します。

<a id="quickstart"></a>
## クイックスタート

1. リポジトリに [`cruise-agent` GitHub App](https://github.com/apps/cruise-agent/installations/new) をインストールします（省略可能です。省略すると workflow の `GITHUB_TOKEN` にフォールバックします。[認証の仕組み](#how-authentication-works)を参照）。
2. リポジトリ secret（Settings -> Secrets and variables -> Actions）に `ANTHROPIC_API_KEY` と `OPENAI_API_KEY` の一方または両方を追加します。jcode には少なくとも 1 つ必要です。
3. [`examples/cruise.yml`](../../examples/cruise.yml) を `.github/workflows/cruise.yml` にコピーします。

その後 issue を開くか、既存 issue に `@cruise plan <what you want>` とコメントします。完全な plan → fix → run → review の流れは[一般的なワークフロー](#typical-workflow)、メンション形式は[コマンド](#commands)、Anthropic/OpenAI 以外の利用方法は[プロバイダー](#providers)、詳しい手順（workflow YAML 全体、`permissions:`、バージョン固定）は[セットアップ](#setup)を参照してください。

<a id="commands"></a>
## コマンド

トリガーフレーズの後の最初の単語（大文字小文字を区別しない。先頭の `/` は任意）でコマンドを選びます。`plan`、`fix`、`exec`、`run` はそのコマンドを選択し、`/tell` は `/` が必須です。単語がない場合、または未知の単語の場合は `run` が既定です。

照合は字句だけで行われるため、`@cruise fix the flaky test` のようにコマンド語から始まる文は `fix` コマンドとして解析されます。自由形式のリクエストには `@cruise /run <request>` や `@cruise /exec <request>` のスラッシュ形式を使ってください。本文に `@cruise` メンションが複数ある場合は、最後のメンションだけを使用します。

| メンション | コマンド | 動作 |
|---|---|---|
| `@cruise`、`@cruise run <request>`、`@cruise /run` | **run** | 計画を解決し（下記参照。`run` の後に入力した文は追加指示として追記）、その内容を変更せずセッションを作成し（LLM による計画生成は行わず）、worktree 内で実行します。既定では実行が成功し branch commit が残れば draft pull request を作成します。 |
| `@cruise exec <request>`、`@cruise /exec` | **exec** | `run` と同様に計画を解決し（追加指示も含む）、すでに checkout された default branch 上で直接実行して、その branch に **直接 push** します（PR なし）。高度なオプトイン機能です。[exec の注意点](#exec-caveats)を参照してください。 |
| `@cruise plan <request>`、`@cruise /plan` | **plan** | issue のタイトルと本文を使って LLM 計画呼び出し（`cruise plan`）を実行し、結果を新しい **計画追跡コメント**として投稿します。実行はしません。 |
| `@cruise fix <feedback>`、`@cruise /fix <feedback>` | **fix** | 直近の *信頼できる* 計画追跡コメントを `<feedback>` に沿って改訂し、**同じコメントをその場で編集**して更新済み計画に置き換えます。既存の計画コメントがなければ、明確なエラーを返します。 |
| `@cruise /tell <instruction>` | **tell** | この issue に対する実行中の `run` job に指示を送信します。issue コメントからのみ使用でき、run は開始しません。 |

**計画の解決**（`run` と `exec` で使用）: issue のすべてのコメントを調べ、`<!-- cruise:plan -->` marker を含み、固定許可リストにある login（`cruise-agent[bot]` または `github-actions[bot]`）の **bot アカウント**（`user.type == "Bot"`）が投稿した最後のコメントを探します。それ以外の identity（PAT や独自 App token を含む）で投稿されたコメントは再利用せず、issue 自体のタイトルと本文にフォールバックします。信頼できる計画コメントがあればその内容を、なければ issue のタイトルと本文を使います。どちらの場合も、トリガーとなったコメント／issue 内のコマンド語の後に書かれた文章を「## Additional instructions from the triggering comment」セクションとして追記します。

<a id="typical-workflow"></a>
## 一般的なワークフロー

計画を先に行う場合の、最初から最後までの例です。

1. **issue を開く** — タスクを説明します。後述の警告にある理由から、issue のタイトルと本文にはトリガーフレーズ（`@cruise`）を含めないでください。

   > **Add retry logic to the uploader**
   >
   > The S3 upload occasionally fails on flaky networks. It should retry with exponential backoff instead of failing the whole job.

2. **計画を依頼する** — `@cruise plan` とコメントします。Action が LLM 計画呼び出しを行って計画追跡コメントを新規投稿します。まだ実行はされません。

3. **計画を必要なだけ調整する** — `@cruise fix also add a changelog entry and cover the timeout case` とコメントします。Action は同じ追跡コメントをその場で編集して更新します。

4. **実行する** — `@cruise run` とコメントします。Action は（必要に応じて修正済みの）計画からセッションを作り、worktree 内で実装します。実行が成功して branch commit が残り、自動 PR 作成が有効であれば draft PR を作成します。commit がない成功実行、または `create_pr: false` では PR なしで完了します。

5. **レビューして ready にする** — PR が作成された場合、どのコマンドで作成されたかにかかわらず、Cruise は必ず **draft** として開きます。PR を開いて diff をレビューし、その後「Ready for review」に変更（または `gh pr ready <number>` を実行）してから merge してください。commit がない場合や PR なしで成功した場合はレビュー対象の PR はありません。

> [!WARNING]
> issue 自体のタイトルまたは本文にトリガーフレーズがすでに含まれる場合、`issues: [opened]` イベントが直ちに発火します。計画を先にレビューしたい場合は issue にトリガーフレーズを含めず、後続コメントにだけ入力してください。

同じ issue に対する複数のメンションは競合せずキューに入ります。前の実行の完了を待たずに続けてコメントしても安全です。

`run` job が実行中は、権限のある issue コメントを確認します。受理／拒否されたコメントには eyes reaction が付きます。受理された文章は現在実行中の prompt ではなく、次の agent prompt 用にキューされます。

**tell に応答がない場合:** コメントに `/tell` を使ったか、投稿者に権限があるか、その issue の `run` job が実行中だったかを確認してください。job log と reaction を確認します。

<a id="setup"></a>
## セットアップ

1. リポジトリに [`cruise-agent` GitHub App](https://github.com/apps/cruise-agent/installations/new) をインストールします。Action が workflow 独自の `GITHUB_TOKEN` ではなく、scope を限定した bot identity（`cruise-agent[bot]`）として認証できるようになります。詳しくは下記の[認証の仕組み](#how-authentication-works)を参照してください。この手順を省略しても Action は動作しますが、説明された制限のある `GITHUB_TOKEN` にフォールバックします。
2. リポジトリに `ANTHROPIC_API_KEY` および／または `OPENAI_API_KEY` secret を追加します（Settings -> Secrets and variables -> Actions）。少なくとも 1 つ必要です。
3. [`examples/cruise.yml`](../../examples/cruise.yml) を `.github/workflows/cruise.yml` にコピーします（下記は抜粋です）。

   ```yaml
   name: Cruise

   on:
     issue_comment:
       types: [created]
     issues:
       types: [opened]

   jobs:
     cruise:
       if: |
         (github.event_name == 'issue_comment' && !github.event.issue.pull_request && contains(github.event.comment.body, '@cruise')) ||
         (github.event_name == 'issues' && (contains(github.event.issue.title, '@cruise') || contains(github.event.issue.body, '@cruise')))
       runs-on: ubuntu-latest
       timeout-minutes: 60
       permissions:
         contents: write
         pull-requests: write
         issues: write
         id-token: write # needed for the cruise-agent App token exchange; optional
       steps:
         - uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7
           with:
             fetch-depth: 0
         - uses: smartcrabai/cruise@v1
           with:
             anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
   ```

4. `@cruise` に言及した issue を開くか、既存 issue に `@cruise <command> ...` とコメントします。

<a id="providers"></a>
## プロバイダー

この Action は既定の `sdk: jcode` backend を提供します（[実行の仕組み](#sdk-jcode----how-execution-works)を参照）。repository config が別の backend を指定している場合はその設定が維持されます。jcode には数十種類の provider があり、いずれも必要なのは API key です。指定方法は次の 3 つです。

- **専用 input** — `anthropic_api_key` / `openai_api_key` は、jcode の `anthropic-api` / `openai-api` credential として登録されます。
- **`env`** — その他の provider key（例: `KIMI_API_KEY`、`GROQ_API_KEY`、`OPENROUTER_API_KEY`）を実行環境に export すると、jcode が環境から取得します。
- **`providers` と `provider_api_keys`** — jcode が標準で知らない OpenAI 互換 endpoint（セルフホスト gateway や proxy など）では、各 `providers` 項目から `[providers.<name>]` profile を作成し、`jcode provider add` で登録します。

```yaml
with:
  providers: |
    {
      "my-gateway": {
        "base_url": "https://gateway.example/v1",
        "model": "my-custom-model",
        "context_window": 200000,
        "default": true
      }
    }
  provider_api_keys: |
    my-gateway=${{ secrets.MY_GATEWAY_KEY }}
```

profile の key は `jcode provider add --api-key-stdin` に渡され、job の jcode home 内に owner のみ読める env file として保存されます。`config.toml` に書かれるのは環境変数名だけで、Cruise config file に key が書き込まれることはありません。

<a id="provider-profile-schema"></a>
### Provider profile のスキーマ

`providers` の各 key は profile 名（ASCII 英字、数字、`-`、`_` のみ。先頭は英字または数字で、最大 64 文字）で、その値は次の項目を含む object です。

- **`base_url`**（必須）— endpoint URL。
- **`model`**（必須）— profile の既定 model ID。
- **`context_window`**（任意）— 数値。
- **`auth`**（任意）— `bearer`（既定）または `api-key`。
- **`auth_header`**（任意）— header 名。`auth: api-key` が必要です。
- **`provider_routing`**（任意、Boolean）— OpenRouter 形式の gateway 用。
- **`no_auth`**（任意、Boolean）— endpoint が一切 credential を要求しない（ローカル proxy、開発用 gateway など）。`no_auth` 項目を `provider_api_keys` に含めてはいけません。すべての `providers` 項目が `no_auth` の場合、`provider_api_keys` を完全に空にできます。
- **`default`**（任意、Boolean）— jcode の起動時既定 provider/model にします。有効にできる項目は最大 1 つです。

この一覧にない key は、その名前を示すエラーとともに拒否されます。typo が黙って破棄されるのを防ぐためです。

<a id="built-in-providers"></a>
### 組み込みプロバイダー

| Provider | jcode CLI credential provider | 環境変数 |
|---|---|---|
| Anthropic | `anthropic-api` | `ANTHROPIC_API_KEY`（専用 `anthropic_api_key` input） |
| OpenAI | `openai-api` | `OPENAI_API_KEY`（専用 `openai_api_key` input） |
| Kimi for Coding | `kimi` | `KIMI_API_KEY`（`env` 経由） |
| Google Gemini | `gemini-api` | `GEMINI_API_KEY`（`env` 経由） |
| Groq | `groq` | `GROQ_API_KEY`（`env` 経由） |
| Mistral AI | `mistral` | `MISTRAL_API_KEY`（`env` 経由） |
| DeepSeek | `deepseek` | `DEEPSEEK_API_KEY`（`env` 経由） |
| xAI (Grok) | `xai` | `XAI_API_KEY`（`env` 経由） |
| Moonshot AI | `moonshot-ai` | `MOONSHOT_API_KEY`（`env` 経由） |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY`（`env` 経由） |

この表の識別子は CLI による credential 設定用です。受け入れ可能な ID は `jcode login --help` に表示されます。カスタム endpoint profile は `providers` / `provider_api_keys` と `jcode provider add` で登録します。

<a id="kimi-for-coding-example"></a>
### Kimi for Coding の例

[Kimi for Coding](https://api.kimi.com/coding/) は jcode CLI の credential provider `kimi` を使い、`KIMI_API_KEY` で認証します。Cruise が使用する互換 model は、Action の `model` input または repository の workflow／`.cruise/default.yaml` に指定してください。

```yaml
- uses: smartcrabai/cruise@v1
  with:
    # Set model to a route available for this credential, or configure it in
    # the repository workflow / .cruise/default.yaml.
    env: |
      KIMI_API_KEY=${{ secrets.KIMI_API_KEY }}
```

<a id="sdk-jcode----how-execution-works"></a>
## sdk: jcode — 実行の仕組み

Cruise は repository workflow config と、適用対象の checkout プロジェクト既定値から backend を解決します。repository workflow config に `command:` または `sdk: claude` があればその設定が維持されるため、その backend 用 CLI と credential を用意してください。

- **OpenAI service tier** は workflow またはプロセス環境に `JCODE_OPENAI_SERVICE_TIER` が定義されていない場合、既定で `off` です。明示された値はそのまま尊重されます。
- **Model input**（`model` / `plan_model`）は Cruise の対応設定を上書きします。実際の jcode prompt ごとに、`provider/model[:effort]` または bare-model 形式の有効な Cruise model が必要です。`model` は Action input、repository workflow、checkout の `.cruise/default.yaml` から設定できます。jcode home の既定 model と `JCODE_MODEL` は代わりになりません。計画呼び出しには `plan_model` が使われ、未設定なら `model` にフォールバックします。任意の effort 接尾辞では `low|medium|high|xhigh|max` と、[SDK モード](sdk.md#sdk-mode)に記載された alias が使えます。

<a id="zero-config-default"></a>
### Zero-config の既定値

`config` が空で、ルート workflow config がなく、`.cruise/` 直下に YAML ファイルもない場合、この Action は `write-tests -> implement` の config を生成します。jcode prompt に必要な model は、適用対象の checkout プロジェクト既定値または Action model input で指定してください。

<a id="config-resolution"></a>
## config の解決

- **`config` input が設定されている** — 絶対パスに解決し、`run` / `plan` / `fix` コマンドで使用します。
- **`config` input が空で、repository に既存 config がある** — cruise 自身の resolver にファイルを選ばせます。

どのケースでも、`model` / `plan_model` input（Cruise 自身が適用する `CRUISE_MODEL` / `CRUISE_PLAN_MODEL` 環境変数 override）が config file の設定有無にかかわらず優先されます。

CI で使う config に `option:` ステップを含めないでください。対話的な入力を求めますが、Actions に terminal はありません。

<a id="env-input"></a>
## `env` input

`env` input で cruise process に追加環境変数を渡します。1 行に `KEY=VALUE` を 1 つ指定します。

```yaml
- uses: smartcrabai/cruise@v1
  with:
    anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
    env: |
      MY_TOOL_TOKEN=${{ secrets.MY_TOOL_TOKEN }}
      FEATURE_FLAG=true
```

空行と `#` で始まる行は無視されます。各値は export 前に mask されます（`::add-mask::`）。予約済みの名前、すなわち `GITHUB_TOKEN`、`GH_TOKEN`、`PATH`、`HOME`、`SHELL`、git identity 変数（`GIT_AUTHOR_*`/`GIT_COMMITTER_*`）、`JCODE_HOME`、`XDG_DATA_HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME`、および `CRUISE_`/`GITHUB_`/`ACTIONS_`/`RUNNER_` で始まる名前は、この Action 自身が管理するため上書きせず `::warning::` を出してスキップされます。raw な `CRUISE_*` 変数の代わりに専用の `model` / `plan_model` / `config` input を使ってください。`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` はここでの予約名ではありません。専用の `anthropic_api_key` / `openai_api_key` input の代わり、または併用で `env` に渡せます。

<a id="exec-caveats"></a>
## exec の注意点

`exec` は PR とレビュー段階を経ずに **default branch へ直接 push** します。そのため次の点に注意してください。

- **branch protection と相性がよくありません。** default branch に PR、status check、レビューが必須の場合、cruise の直接 push はそのまま失敗し、run も失敗になります。`exec` は default branch への直接 push を意図的に許可するリポジトリ、または Cruise の actor/token に保護規則の bypass を許可するリポジトリ向けです。
- **高度なオプトイン機能です。** `git revert` 以外に取り消す方法はありません。無人の直接 push を意図しているのでなければ、レビューできる draft PR を作る `run` を優先してください。
- `cruise exec` でファイル変更が生じなければ commit+push ステップを完全にスキップし、成功として報告します。

直接 push を意図する場合は明示的に `exec` コマンドを使います。

<a id="how-authentication-works"></a>
## 認証の仕組み

既定では（`github_token` input を空のままにすると）、workflow 独自の `GITHUB_TOKEN` ではなく `cruise-agent` GitHub App として認証しようとします。

1. `permissions: id-token: write` が許可されている場合、GitHub Actions は workflow、repository、run を識別する短命な OIDC token を job に渡します。
2. OIDC token を **repository scope に限定された、短命な cruise-agent App installation token** と交換します。呼び出し元の repository に scope された token のみが発行されます。
3. cruise はこの token で `cruise-agent[bot]` として認証し、実行して commit を push します。Action が作成する commit には、メンションを開始したユーザーの GitHub 提供 noreply address を使う `Co-authored-by` trailer も追加されます。

App が repository にインストールされていない、OIDC token を取得できない（例: `id-token: write` が許可されていない）、または exchange service に接続できない場合、Action は **workflow の `GITHUB_TOKEN` にフォールバック**します。その場合も cruise は動きますが、`GITHUB_TOKEN` で作成された draft PR は別の `on: pull_request` workflow を起動せず、commit の author／committer は `github-actions[bot]` になります。

<a id="bring-your-own-token"></a>
## 独自 token の使用

`github_token` に明示的に値（PAT や独自 GitHub App token など）を設定すると OIDC exchange を完全に省略します。Action はすべての GitHub API 呼び出しと push にその token を使います。

```yaml
- uses: smartcrabai/cruise@v1
  with:
    anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
    github_token: ${{ secrets.MY_PAT }}
```

<a id="self-hosting-the-token-exchange"></a>
## Token exchange のセルフホスト

独自の GitHub App と Worker で Token-exchange service を動かす場合、`token_exchange_url` で接続先を指定します。ソースとデプロイ手順は [`../../token-exchange/README.md`](../../token-exchange/README.md) を参照してください。

```yaml
- uses: smartcrabai/cruise@v1
  with:
    anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
    token_exchange_url: https://your-worker.your-subdomain.workers.dev/token
```

`token_exchange_url: ""` にすると exchange を完全に無効化し、常に `github_token` / `GITHUB_TOKEN` を使います。

どちらの場合も Action は起動時に tracking comment を投稿し、終了時に結果で書き換えます。run log は issue コメントへ一切コピーされません。

<a id="inputs"></a>
## Inputs

| Input | 既定値 | 説明 |
|---|---|---|
| `anthropic_api_key` | *(空)* | Anthropic API key。`anthropic_api_key`、`openai_api_key`、`provider_api_keys`、`providers`、`env` の少なくとも 1 つを空でない値にする必要があります。 |
| `openai_api_key` | *(空)* | OpenAI API key。`anthropic_api_key`、`openai_api_key`、`provider_api_keys`、`providers`、`env` の少なくとも 1 つを空でない値にする必要があります。 |
| `github_token` | *(空)* | 権限確認、コメント、PR、push など GitHub API 用 token。空（既定）ならまず cruise-agent App の OIDC token exchange を試み、失敗時は workflow の `GITHUB_TOKEN` にフォールバックします。明示すると exchange を省略してその token を使います。[認証の仕組み](#how-authentication-works)を参照してください。 |
| `token_exchange_url` | `https://cruise-token-exchange.smartcrab.ai/token` | token-exchange service の URL。空にすると exchange を無効にし、常に `github_token` / `GITHUB_TOKEN` にフォールバックします。[Token exchange のセルフホスト](#self-hosting-the-token-exchange)を参照してください。 |
| `trigger_phrase` | `@cruise` | run のトリガーに必要なフレーズ（単語境界で照合）。`issues` イベントでは issue のタイトルと本文を連結して照合するため、タイトル内でも有効です。 |
| `cruise_version` | `latest` | インストールする cruise リリース（`latest` または tag）。 |
| `jcode_version` | `latest` | jcode 独自 installer でインストールする jcode リリース（`latest` または tag）。`jcode` がすでに PATH 上にある場合は無視されます。 |
| `config` | *(空)* | `run` / `plan` / `fix` で使う、repository 内の cruise workflow config YAML のパス（`CRUISE_CONFIG` を設定）。空なら cruise 自身の resolver が checkout 内の config または組み込み `tdd` workflow を選びます。`exec` には影響しません。 |
| `model` | *(空)* | `CRUISE_MODEL` を上書きします。`sdk: jcode` では `provider/model[:effort]`、bare model、または override 未設定を指定できます。実際の jcode prompt ごとに、この input、workflow config、checkout の `.cruise/default.yaml`、または（計画 prompt の場合）`plan_model` / `CRUISE_PLAN_MODEL` から有効な Cruise model を解決する必要があります。jcode の設定済み既定値と `JCODE_MODEL` は代わりになりません。 |
| `flags` | *(空)* | `run` 用ワークフラグ値（空行と `#` で始まる行は無視）。残った各行は 1 つの `--flag` 値として渡され、単独の `NAME`、カンマ区切り、または `NAME=on\|off\|true\|false` の明示的割り当てにできます。input にないフラグはオフです。必須フラグはありません。`exec`、`plan`、`fix` には影響しません。 |
| `plan_model` | *(空)* | `CRUISE_PLAN_MODEL` を上書きします（`plan` / `fix` コマンドの計画ステップ）。形式は `model` と同じです。 |
| `providers` | *(空)* | jcode provider profile 名を key とする JSON object。各項目は `base_url` と `model` が必須で、`context_window`、`auth`、`auth_header`、`provider_routing`、`no_auth`、`default` は任意です。各項目は `jcode provider add` により `[providers.<name>]` profile になります。完全な schema は[プロバイダー](#providers)を参照してください。 |
| `provider_api_keys` | *(空)* | `providers` 用の `profile-name=API key` を 1 行ずつ指定します。空行とコメント行は無視され、最初の `=` が区切りです。各 key は `jcode provider add --api-key-stdin` に渡されます。`no_auth: true` でない `providers` 項目ごとに必要です。すべての項目が `no_auth` の場合は空にできます。 |
| `env` | *(空)* | cruise process に export する追加の `KEY=VALUE` 行（mask 済み）。予約済みの名前は警告とともにスキップされます。 |
| `allowed_bots` | *(空)* | cruise の起動を許可する bot login（`[bot]` を除く）のカンマ区切り。`*` はすべての bot を許可します。空なら bot をすべて拒否します。 |
| `git_user_name` | *(空)* | この Action／cruise が作成する commit の git `user.name`。空なら App token で実行した場合は `cruise-agent[bot]`、それ以外は `github-actions[bot]` になります。 |
| `git_user_email` | *(空)* | commit の git `user.email`。空の場合は `git_user_name` の既定値と対応します。GitHub event payload に起動ユーザーの login と数値 user ID が含まれる場合、commit には引き続きそのユーザーの `Co-authored-by` trailer が追加されます。 |

<a id="outputs"></a>
## Outputs

| Output | 説明 |
|---|---|
| `command` | メンションから解析したコマンドの `"run"`、`"exec"`、`"plan"`、`"fix"`。gate が run をスキップした場合は空です。 |
| `session_id` | コマンド実行で作成された cruise session の ID。 |
| `pr_url` | cruise が作成した pull request の URL（`run` コマンドのみ）。PR が作成されなければ空です。 |
| `commit_url` | cruise が default branch に push した commit の URL（`exec` コマンドのみ）。 |
| `plan_comment_url` | cruise が投稿または編集した計画追跡コメントの URL（`plan` / `fix` コマンドのみ）。 |
| `conclusion` | `success`、`failure`、`skipped`（メンションに一致しなかった、または actor が未許可）。 |
| `used_app` | cruise-agent App installation token で認証した場合は `"true"`、`github_token` / `GITHUB_TOKEN` を使った場合は `"false"`。 |

<a id="security"></a>
## セキュリティ

- **cruise を起動できるのは repository collaborator のみです。** Action はコメントまたはメンションを行ったユーザーに対する GitHub collaborator-permission API を呼び出し、write 権限を要求します（API は `maintain` role を `write` として報告するため maintainer は対象です。`triage` と `read` は対象外です）。明示的に `allowed_bots` へ追加しない限り bot actor は拒否されます。
- **計画コメントは marker だけでなく信頼性も検証します。** `run` / `exec` / `fix` が authoritative な計画ソースとして扱うのは、計画 marker を含み、かつ `cruise-agent[bot]` / `github-actions[bot]` が投稿したコメントだけです。この確認がなければ、write 権限のないコメント投稿者が偽の `<!-- cruise:plan -->` コメントを投稿し、後で maintainer が `@cruise run` と書いたときに未確認の計画が実行される可能性があります（認証された actor が必須なのは *メンション自体* だけであり、公開 issue に対するほかのコメントには必要ありません）。
- **Token exchange が発行するのは repository scope の token のみです。** workflow が取得できるのは実際に実行されている repository に scope された token だけです。`permissions: id-token: write` で可能になるのは job が GitHub に OIDC token を *要求すること* だけです。それだけで GitHub API access が付与されるわけではありません。
- **無人実行です。** cruise は操作ごとの確認 prompt を出しません。prompt injection（下記参照）が成功した際の影響範囲は workflow の GitHub token と runner の権限に及びます。必要な `permissions:`（`contents: write`、`pull-requests: write`、`issues: write`、App token exchange を使う場合は `id-token: write`）だけを付与してください。provider API key、`GITHUB_TOKEN`、この Action が取得する App token は、repository に書き込み可能な他の CI secret と同じように扱ってください。
- **Prompt injection。** issue 本文と fix feedback は攻撃者が制御するテキストであり、モデルに送る prompt に埋め込まれます。Action は GitHub 由来テキストを計画入力にする前に隠れた命令ベクトルを除去します。これは緩和策であり保証ではありません。目に見える普通のテキストで書かれた指示は除去できません。「agent が issue の文章どおりに何でも実行した」結果が許容できる範囲を超える secret や権限を、この Action の workflow に与えないでください。
- **run log を issue に投稿することはありません。** 失敗コメントには log の引用ではなく Actions run へのリンクを載せます。agent の出力には、モデルを誘導して出力させたあらゆる内容（環境値を含む）が含まれる可能性があります。GitHub の secret masking が適用されるのは Actions log viewer だけで、API 経由で再投稿されたテキストには適用されません。公開コメントに log をコピーすると情報流出経路になります。log は repository のアクセス制御が適用される run page に残ります。
- **installer は TLS で取得しますが、チェックサム検証されません。** Action は release installer script（`curl | sh`）で cruise と jcode をインストールします（script 自体の checksum は検証されません）。ダウンロード endpoint が侵害されると job secret を使える攻撃コードが実行されます。`latest` を黙って追跡することを避けるには、少なくとも `cruise_version` / `jcode_version` を tag に固定してください。
- **`GITHUB_TOKEN` で開いた PR は別 workflow を起動しません。** これは意図された GitHub Actions の再帰防止規則で、通常の `GITHUB_TOKEN` フォールバック経路に限って適用されます。cruise-agent App installation token（App がインストールされている場合の既定）で行った PR／push は、GitHub が App 認証の操作を別 actor として扱うため、通常どおり `on: pull_request` workflow を起動します。`GITHUB_TOKEN` フォールバック中に cruise の PR で CI を走らせるには、App をインストールするか、`github_token` 経由で独自 PAT／GitHub App token を使うか、PR を閉じて再度開くステップを追加してください。

<a id="troubleshooting"></a>
## トラブルシューティング

- **`@cruise` にメンションしても何も起こらない。** workflow run list で skipped／no-op run を確認してください。gate ステップの log に、event 種類、action、トリガーフレーズの欠落、PR コメント、actor 権限不足など、拒否した理由が記録されています。
- **「'anthropic_api_key', 'openai_api_key', 'provider_api_keys', 'providers', and 'env' are all empty」。** 少なくとも 1 つの専用 key、生成 profile 用の `providers` / `provider_api_keys`、または `env` 経由の provider key（例: `KIMI_API_KEY=...`）を設定してください。
- **「actor '&lt;login&gt;' has insufficient permission: '&lt;permission&gt;'」。** コメント投稿者には repository の `write`、`maintain`、`admin` access が必要です。
- **`fix` で「No existing plan comment found」。** 先に `@cruise plan` を実行してください。`fix` は既存の計画追跡コメントを編集するだけで、新規作成はしません。
- **`run` で「cruise completed but no pull request was created」。** `permissions: pull-requests: write` と branch protection／repository 規則を確認してください。
- **`exec` の push が失敗する。** ほとんどの場合、default branch の branch protection が原因です。[exec の注意点](#exec-caveats)を参照してください。
- **Model 解決エラー。** 実際の jcode prompt ごとに有効な Cruise model が解決されることを確認してください。jcode の設定済み既定値と `JCODE_MODEL` は代わりになりません。prompt に応じて `model` / `plan_model` input、workflow、checkout の `.cruise/default.yaml` に設定してください。
- **常に `GITHUB_TOKEN` にフォールバックし、`used_app` output が `false`。** 次の順に確認してください。`cruise-agent` App がこの repository にインストール済みであること（[インストールリンク](https://github.com/apps/cruise-agent/installations/new)）、workflow に `permissions: id-token: write` があること、`token_exchange_url` が空ではなく到達可能であること。`token` ステップの log に失敗理由が記録されています。
- **Self-hosted runner** では `git`、`curl`、`jq`、`python3`、`gh` CLI が PATH 上に必要です（GitHub-hosted runner にはすべて事前インストールされています）。

<a id="faq"></a>
### FAQ

- **新しく作った issue が、最初に計画を投稿する代わりにすぐ PR になったのはなぜですか。** issue のタイトルまたは本文にトリガーフレーズがすでに含まれ、`issues: [opened]` event が即時発火したためです。まず計画をレビューしたい場合は issue 本体にトリガーフレーズを入れず、その後 `@cruise plan` とコメントしてください。
- **cruise が開いた pull request を merge できない／ready と表示されない。** `run` からの PR はすべて **draft** で開かれます。diff をレビューしたら、自分で「Ready for review」にしてください。[一般的なワークフロー](#typical-workflow)を参照してください。
- **run が timeout した。** [`examples/cruise.yml`](../../examples/cruise.yml) と同じ `timeout-minutes: 60` を推奨します。テスト suite が遅い場合はさらに延長してください。
- **同じ issue に対する 2 回目の `@cruise` コメントが止まっていて実行されないように見える。** 失われたのではなく、キューに入っています。先行 run がまだ実行中の可能性があるため、Actions run list を確認してください。
