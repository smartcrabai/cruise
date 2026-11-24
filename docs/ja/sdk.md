[English](../en/sdk.md) · [日本語](../ja/sdk.md) · [简体中文](../zh/sdk.md) · [目次](README.md)

# SDK backend

<a id="sdk-mode"></a>
## SDK mode

外部 CLI を `command` で起動する代わりに、top-level の `sdk` field で jcode SDK を選択して prompt step を実行できます。`command` と `sdk` は相互排他です。適用される `default.yaml` layer を解決した後に両方とも設定されていない場合、既定の `jcode` backend が使われます。対応する SDK 値は `jcode` のみです。それ以外は validation error になります。

<a id="startup-oauth-expiration-checks"></a>
### 起動時の OAuth 有効期限チェック

agent 起動前に、Cruise は該当する明示的な jcode Claude / OpenAI OAuth candidate の credential file を確認します。access token が期限切れで refresh token がないか、既知の refresh token 有効期限も切れている場合に停止します。再ログインには `jcode login claude` または `jcode login openai` を使います。

これは有効期限の事前検査であり、認証成功の証明ではありません。誤検知を回避するには process 環境変数 `CRUISE_SKIP_AGENT_AUTH_CHECK=1` を設定してください。正確な値 `1` のみが回避し、`true` や空白付き値では回避されません。

jcode の `openai` と `claude` を明示した model では、残り有効期限が2時間を切ると、Cruise は各 attempt の前にユーザーの jcode home で login を refresh します。これにより、attempt ごとの private runtime が共有 refresh token を自分で回転させることはありません。refresh に失敗し、access token が10分以内に期限切れになる場合、attempt は `jcode login` の案内付きで失敗します。

```yaml
sdk: jcode        # Explicitly select jcode. If omitted, it is the fallback only
                  # when neither `command` nor `sdk` remains set in the resolved
                  # workflow after applicable `default.yaml` layers are applied.

# Every actual jcode prompt needs an effective model. Use
# `provider/model[:effort]` or a bare model available in your jcode setup.
# model: provider/model[:effort]
```

`model`、`plan_model`、step-level `model` は jcode の **model reference** です。優先順位は command mode と同じです（step `model` > top-level `model` / `plan_model`）。`provider/model[:effort]`（例: `claude/claude-opus-4-6`）または bare model を指定します。省略可能な `:effort` suffix は推論 effort tier を選択します。`low` / `medium` / `high` / `xhigh` / `max`、alias の `minimal` / `min` / `med`、数値表記 `1`..`4` が使えます。`off` / `none` / `0` / `5` は effort 未設定として扱われ、それ以外の suffix（例: OpenRouter の `:free`）は model ID の一部として残ります。provider または model が未対応の effort は無視されます。

workflow-level `model` と `plan_model` は文字列、または文字列と `{model, disabled_during}` object を含む array にできます。先頭が primary model で、後続 entry は fallback chain です。fallback entry を持つ array は自動的に model fallback を有効にします。明示的な文字列のみの `retry.fallback_chains` entry は fallback の順序で優先されます。`sdk` を含む object は使用できず、`SDK-qualified model objects are unsupported; use a plain model string` という validation error になります。

対応する model reference:

- `"provider/model[:effort]"`（例: `provider/model:xhigh`）— jcode は provider と model を分けて parse します。`"/model"` や `"provider/"` のように片側が空なら prompt 実行時に失敗します。
- `"model"`（`/` なし）— 明示的な provider は指定しません。
- 未設定 — 有効な model が残っていない jcode prompt は setup error で失敗します。`command:` は従来どおり model なしで動作します。

<a id="subagent-model"></a>
### subagent model

workflow-level または prompt step に `subagent_model` を設定して、spawn する agent の model を選びます。step 値は workflow 値を上書きします。空でない単一の model 文字列のみ指定でき、object、array、fallback chain は受け付けません。jcode prompt で使用され、`command:` backend では利用できません。

```yaml
sdk: jcode
model: openai/gpt-6.1-sol
subagent_model: openai/gpt-6.1-sol
steps:
  review:
    prompt: Review the implementation, delegating checks as needed.
    subagent_model: anthropic/claude-sonnet-4-6
```

jcode の subagent model は有効な空でない model に解決される必要があります。

<a id="advisor-steps"></a>
### アドバイザーステップ

`advisor: true` は `steps` 直下の通常の prompt step（`workflow_call` でそこに展開されたステップを含む）に指定できます。`output_file` が必須で、`allow_commit: true` は指定できません。ワークフローの既定値に関係なく[読み取り専用](#permission-modes)になります。ステップの `permission` は省略するか `read-only` にしてください。

モデルはステップの `model` → `advisor_model` → `plan_model` → ワークフローの `model` の順で選択します。空のロール配列は未設定として扱います。`advisor_model` は `plan_model` と同様、モデル文字列または `disabled_during` を含められるフォールバック配列を指定できます。ワークフローやユーザー／プロジェクトの `default.yaml` に設定し、`CRUISE_ADVISOR_MODEL`（文字列または JSON 配列）で上書きできます。`cruise config` の設定項目ではなく、他の prompt には影響しません。

有効なアドバイザーには jcode が必要です。実行前の検証（dry run を含む）では、フラグで無効化されたアドバイザーと、ユーザーが明示的にスキップしたアドバイザーをこの確認から除外します。`cruise workflow doctor` が適用するのはフラグによるスキップのみです。アドバイザーは `cruise tell` で保留中の指示を消費しません。

実行前に Cruise はセッションの成果物 `<output_file>.diff` を生成します。`{file:<output_file>.diff}` で読み込めます。内容は統計に続く `HEAD` からの未コミット差分です。無視されていない未追跡ファイルを含み、`.worktreeinclude` に記載されたパスは除外します。256 KiB を超える場合は行の境界で切り詰め、その旨を追記します。試行前やスキップ時には古い助言を削除し、成功した場合だけ新しい助言を保存します。助言がないのに `{file:<output_file>}` で読み込むとエラーになります。

差分取得失敗、バックエンドエラー、タイムアウトなどの通常の失敗は `if.fail` で処理できます。作業領域の変更を検出した場合、検証できない場合、またはユーザーが中断した場合は、`if.fail` に関係なく実行を停止します。助言と差分は信頼できない参照データであり、指示や制御フロー、承認の入力には使いません。

<a id="sdk-jcode--the-jcode-sdk-default"></a>
### `sdk: jcode` — jcode SDK（既定）

`model`、`plan_model`、step-level `model` field には `provider/model[:effort]` または bare model を指定します。実際に実行するすべての jcode prompt で model が解決されている必要があります。通常の prompt は step `model`、次に workflow/default `model` を使います。planning call は `plan_model`、次に `model` を使います。model がなければ user `default.yaml` template を作成または保持し、その要求を停止します。effort suffix は SDK 経由で適用されます。

<a id="computer-use"></a>
#### Computer use

workflow-level `computer_use` の既定値は `false` です。`true` にすると macOS 上の prompt turn で jcode の `macos_computer_use` desktop-control tool を許可します。prompt step では `computer_use: true` または `false` を指定して workflow default を上書きできます。この tool は macOS 専用で、Linux では設定の効果はありません。ユーザーには macOS の Accessibility と Screen Recording permission が必要です。

<a id="permission-modes"></a>
### permission mode

workflow-level `permission` の既定値は `full` です。prompt step に `permission: read-only | edit | full` を設定して上書きできます。`edit` は shell 実行を無効にしますが、直接の edit/write/patch tool は維持します。`read-only` はさらに file 変更 tool を無効にします。制限 mode では `computer_use: true` でも `macos_computer_use` を強制的に無効化し、mode 別 tool 設定を選択された `JCODE_DISABLED_TOOLS` に merge します。

- Cruise は任意の CLI tool を制御できないため、`command:` backend では `read-only` と `edit` を拒否します。
- `read-only` と `allow_commit: true` の併用は設定エラーです。`allow_commit: true` は HEAD/ref commit guard と暗黙の Git 拒否だけを解除します。明示的制限は解除せず、permission mode を広げません。
- group call、`workflow_call`、parallel wrapper には `permission` を設定できません。内側の prompt または parallel prompt child に設定してください。read-only child が1つでもある parallel block は、read-only prompt child のみで構成する必要があります。
- read-only step は step 実行中に workspace snapshot が変わると `ReadOnlyWorkspaceChanged` で失敗します。これは変更検出であり、OS sandbox や rollback ではありません。`.git`、`target`、`node_modules` は対象外です。

<a id="tool-restrictions"></a>
### tool の制限

`disallowed_tools` は権限を付与せずに SDK prompt の機能を狭めます。

```yaml
disallowed_tools: ["shell:npm publish"]
steps:
  review:
    prompt: Review without publishing or delegating.
    disallowed_tools: [web, subagent, "mcp:github", "shell:gh pr merge"]
```

| 値 | 拒否される機能 |
|-------|-------------------|
| `web` | Web fetch/search、および利用可能な backend browser tool |
| `subagent` | subagent または swarm への委譲 |
| `mcp` | workflow の全 `mcp_servers` entry |
| `mcp:<name>` | 指定した workflow MCP server。未知の名前はエラー |
| `shell:<prefix>` | 先頭の単語が prefix と一致する単純な shell command |

これらは SDK 中立の値です。shell/edit 全体を制限するには `permission` を使います。裸の `shell` と `edit` は無効です。shell prefix は trim され、連続する空白は1つにまとめられます。空でないことが必要で、ASCII 英字、数字、空白、`._/:=@+%-` のみ使用できます。wildcard、quote、comma、括弧、shell operator、backslash、改行、制御文字は拒否されます。最後の単語が `=` で終わる場合、その単語の先頭部分に一致します。例: `shell:git -c core.hooksPath=`。

有効な list は workflow default、step 制限、暗黙の Git rule の**重複を除いた union**です。step の `[]` では workflow 制限を解除できません。user/project `default.yaml` と workflow layer 間では、最も優先度の高い明示 workflow-level list が下位 list を置き換えます。workflow の `disallowed_tools: []` は継承 default を解除します。`edit` または `read-only` では shell は既に無効なので shell rule は除去されます。

空でない list は prompt のみに適用されます。parallel prompt child、group 内または展開された workflow 内 prompt、`after-pr`、`github-review` loop を含みます。command/option step、parallel wrapper、group と `workflow_call` の call site、native `squash-merge` には設定できません。呼び出された workflow の step list は保持されますが、top-level list は無視され caller の設定が使われます。`command:` prompt backend は明示制限を拒否し、暗黙 shell rule も強制しません。既存の commit guard は引き続き動作します。

`allow_commit: true` でない限り、SDK prompt には次の shell prefix が暗黙に拒否されます。

```text
git commit                 git merge                  git rebase
git cherry-pick            git revert                 git am
git pull                   git push                   git reset
git switch                 git checkout -b            git checkout -B
git update-ref             git config core.hooksPath  git -c core.hooksPath=
```

これには `git reset HEAD file` も含まれます。unstage には `git restore --staged` を使ってください。`git checkout -- file`、`git stash create`、`git branch` による一覧表示、`git status`/`diff`/`log` は拒否されません。prefix matching では安全な Git 操作と危険なものすべてを区別できないため、最後の防御として commit guard も残ります。repository に限定された commit guard と異なり、shell prefix 拒否は一時 repository や無関係な repository を対象とする一致 command にも適用されます。

MCP 制限が filter するのは backend 起動前の workflow 提供 entry のみです。user-level backend MCP 設定と Cruise 独自 session tool は有効なままです。jcode では workflow または process 環境変数 `JCODE_HOOKS_DISABLED` で shell gate を無効化でき、hook の実行失敗時も fail-open になります。これらの制御は多層防御であり sandbox ではありません。prompt warning、reference-transaction hook、CAS rollback も引き続き適用されます。

Bubblewrap による隔離は `jcode-bubblewrap` feature を有効にした Linux build でのみ利用できます。通常の jcode 試行は daemon 起動時に `CRUISE_JCODE_BUBBLEWRAP=1` が設定された場合に要求し、workflow の `require_bubblewrap: true` はこの環境変数に関係なく要求します。Cruise は process ごとに一度、利用可否を probe します。feature がない build、Linux 以外の host、または probe の失敗時、要求された attempt は隔離なしで実行され warning が表示されます。`command:` backend も許可され、隔離なしで実行され warning が表示されます。probe 成功後に実際の起動が失敗した場合は attempt が失敗します。隔離された各 attempt は専用 daemon を起動し、Git metadata を読み取り専用にします。`GIT_OPTIONAL_LOCKS=0` で `git status` / `git diff` / `git log` は使用可能。stage、commit、push はホストが行います。ホスト側で index を置換した場合、変更が見えるのは次の attempt です。共有 `jcode serve` には接続しません。


<a id="tool-less-non-interactive-planning"></a>
### tool を使わない（非対話）planning

SDK mode の planning は既定で session tool を使い、質問、plan の提出／修正を行います。

`interactive_planning: false` を設定すると、その動作を無効にします。planning prompt に対象 plan file path を埋め込み、agent に `plan.md` を直接書かせます。これは `command` backend と同じ方式です。完了後に `plan.md` を読み込み、file が作成されていなければ agent が捕捉した output を fallback として使います。

```yaml
interactive_planning: false   # tool-less, file-based planning
```

`--grill` には interactive planning flow が必要であり、`interactive_planning` が無効なら拒否されます。常に file-based の `command` mode ではこの field の効果はありません。


<a id="mcp-servers-mcp_servers"></a>
### MCP server (`mcp_servers`)

省略可能な top-level `mcp_servers` map は、すべての jcode prompt run に MCP server を提供します。workflow prompt step（parallel child を含む）、`after-pr` step、planning turn、session title 生成、PR-description 生成が対象です。step-level override はありませんが、prompt の `disallowed_tools: [mcp]` または `["mcp:<name>"]` で workflow entry を除外できます。user-level MCP 設定には影響しません。呼び出された workflow の top-level `mcp_servers` は無視され、caller の設定が維持されます。

entry は MCP server の形式です。ローカル server では `type` を省略するか `type: stdio` を指定します。HTTP と SSE transport は非対応で、config validation error になります。`command` backend には MCP server を渡せません。

```yaml
sdk: jcode
mcp_servers:
  local_search:
    command: npx
    args: ["-y", "@example/search"]
```

MCP entry の値は Cruise の `{variable}` template 解決を受けず、そのまま渡されます。名前は空でない必要があり、ASCII 英数字、`_`、`-` のみ使用できます。`cruise` は予約済みです。stdio entry は空でない `command` が必要で、`url` や空でない `headers` は指定できません。未知の entry field は拒否されます。`command` backend に空でない `mcp_servers` map がある場合、validation error です。
