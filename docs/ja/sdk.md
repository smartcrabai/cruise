[English](../en/sdk.md) · [日本語](../ja/sdk.md) · [简体中文](../zh/sdk.md) · [目次](README.md)

# SDK backend

<a id="sdk-mode"></a>
## SDK mode

外部 CLI を `command` で起動する代わりに、top-level の `sdk` field を設定して SDK backend で prompt step を実行できます。`command` と `sdk` は相互排他です。適用される `default.yaml` layer を解決した後に `command` も `sdk` も残っていない場合に限り、既定の `jcode` backend が使われます。受け付ける値は `jcode`、`claude`、`opencode` の3つです。

<a id="startup-oauth-expiration-checks"></a>
### 起動時の OAuth 有効期限チェック

Cruise は agent を起動する前に、該当する SDK のローカル OAuth credential file の有効期限を確認します。access token が期限切れで、refresh token がないか、記録された refresh token の有効期限が現在時刻以前の場合に限り Cruise は停止します。再ログインには必要に応じて `claude auth login`、`jcode login claude`、`jcode login openai` を使います。

これは有効期限の事前検査であり、認証成功の証明ではありません。誤検知を回避するには process 環境変数 `CRUISE_SKIP_AGENT_AUTH_CHECK=1` を設定してください。正確な値 `1` のみが回避し、`true` や空白付き値では回避されません。

```yaml
sdk: jcode        # Explicitly select jcode. If omitted, it is the fallback only
                  # when neither `command` nor `sdk` remains set in the resolved
                  # workflow after applicable `default.yaml` layers are applied.

# Every actual jcode prompt needs an effective model. Use
# `provider/model[:effort]` or a bare model available in your jcode setup.
# model: provider/model[:effort]
```

すべての SDK backend で `model` / `plan_model` / step-level `model` は **model reference** であり、command mode と同じ override 優先順位（step `model` > top-level `model` / `plan_model`）が適用されます。jcode と Claude では、省略可能な `:effort` suffix が推論 effort tier を選択します。`low` / `medium` / `high` / `xhigh` / `max` と alias `minimal` / `min` / `med`、数値表記 `1`..`4` が使えます。`off` / `none` / `0` / `5` も消費されますが effort は未設定のままです。それ以外の `:` suffix（例: OpenRouter の `:free` variant）は model ID の一部として残ります。jcode/Claude では provider または model が未対応の effort は無視されます。OpenCode は catalog に照らして effort を検証し、非対応なら明示的に失敗します。

workflow-level `model` と `plan_model` は array にすることもできます。SDK mode では最初の entry が primary model、後続 entry が暗黙の fallback chain になります。fallback entry を持つ model array は自動的に model fallback を有効にします。primary model に対する明示的な `retry.fallback_chains` entry は array の残りより優先されます。

対応する model reference:

- `"provider/model[:effort]"`（例: `provider/model:xhigh`）— `sdk: jcode` は provider と model を分けて parse します。`"/model"` や `"provider/"` のように片側が空なら prompt 実行時に失敗します。`sdk: claude` は `:effort` suffix 以外すべてを model 名として扱います。
- `"model"`（`/` なし）— jcode/Claude では明示 provider を指定しません。OpenCode の設定済み reference には provider が必要です。
- `"provider/model[:effort]"` または `"provider/model#variant"` — `sdk: opencode` 用です。model ID 内の `/` と `:free`、`:2026` のような未知の colon suffix はそのまま残ります。認識される末尾 effort suffix は予約済みで、`#variant` と組み合わせられません。
- 未設定 — 明示 model を渡しません。OpenCode は設定済み default を使います。有効 model が依然として存在しない jcode prompt は setup error で失敗します。Claude と `command:` は従来どおり model なしで動作します。

<a id="subagent-model"></a>
### subagent model

workflow-level または prompt step に `subagent_model` を設定して、spawn する agent の model を選びます。step 値は workflow 値を上書きします。値は空白でない単一文字列、または `{sdk: claude, model: haiku}`（`sdk: jcode` も可）です。array や fallback chain は受け付けません。

```yaml
sdk: claude
model: sonnet
subagent_model: haiku
steps:
  review:
    prompt: Review the implementation, delegating checks as needed.
    subagent_model: {sdk: claude, model: sonnet}
```

- OpenCode の subagent model と `command:` backend は非対応です。mixed-SDK workflow では明示的な SDK object で Claude/jcode candidate を対象にできます。認識される `:effort` suffix は `:off` も含め拒否されます。`:free` など未知の suffix は model ID の一部です。jcode では有効な空でない model が必要です。

<a id="sdk-jcode--the-jcode-sdk-default"></a>
### `sdk: jcode` — jcode SDK（既定）

`model`、`plan_model`、step-level `model` field には `provider/model[:effort]` または bare model を指定します。実際に実行するすべての jcode prompt で model が解決されている必要があります。通常の prompt は step `model`、次に workflow/default `model` を使います。planning call は `plan_model`、次に `model` を使います。model がなければ user `default.yaml` template を作成または保持し、その要求を停止します。effort suffix は SDK 経由で適用されます。

<a id="computer-use"></a>
#### Computer use

workflow-level `computer_use` の既定値は `false` です。`true` にすると macOS 上の prompt turn で jcode の `macos_computer_use` desktop-control tool を許可します。prompt step では `computer_use: true` または `false` を指定して workflow default を上書きできます。この tool は macOS 専用で、Linux では設定の効果はありません。ユーザーには macOS の Accessibility と Screen Recording permission が必要です。

<a id="permission-modes"></a>
### permission mode

workflow-level `permission` の既定値は `full` です。prompt step に `permission: read-only | edit | full` を設定して上書きできます。`edit` は shell 実行を無効にしますが、直接の edit/write/patch tool は維持します。`read-only` はさらに file 変更 tool を無効にします。制限 mode では `computer_use: true` でも `macos_computer_use` を強制的に無効化します。

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

これらは SDK 中立の値であり、Claude の `Bash(...)` 構文ではありません。shell/edit 全体を制限するには `permission` を使います。裸の `shell` と `edit` は無効です。shell prefix は trim され、連続する空白は1つにまとめられます。空でないことが必要で、ASCII 英字、数字、空白、`._/:=@+%-` のみ使用できます。wildcard、quote、comma、括弧、shell operator、backslash、改行、制御文字は拒否されます。最後の単語が `=` で終わる場合、その単語の先頭部分に一致します。例: `shell:git -c core.hooksPath=`。

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

MCP 制限が filter するのは backend 起動前の workflow 提供 entry のみです。user-level backend MCP 設定と Cruise 独自 session tool は有効なままで影響を受けません。Claude の `shell:<prefix>` 拒否は Git global option を正規化しないため、たとえば `git -C path commit` が拒否されないことがあります。jcode では workflow または process 環境変数 `JCODE_HOOKS_DISABLED` で shell gate を無効化でき、hook の実行失敗時も fail-open になります。これらの制御は多層防御であり sandbox ではありません。prompt warning、reference-transaction hook、CAS rollback も引き続き適用されます。

<a id="sdk-claude--the-claude-cli-in-process"></a>
### `sdk: claude` — in-process の claude CLI

`sdk: claude` は claude-agent-sdk を通じて `claude` CLI を in-process で実行します。model reference は通常の `claude --model` 名で、省略可能な `:effort` suffix は `--effort` として渡されます。認証には claude CLI 固有の保存済み credential または `ANTHROPIC_API_KEY` を使い、`jcode login` とは無関係です。Cruise workflow は無人実行のため CLI は permission bypass 付きで動作します。そのため permission prompt に応答する console はありません。Claude Code の computer use は interactive session でしか使えず Cruise から有効化できないため、`computer_use: true` は config load 時に拒否されます。

<a id="tool-less-non-interactive-planning"></a>
### tool を使わない（非対話）planning

SDK mode の planning は既定で session tool を使い、質問、plan の提出／修正を行います。

`interactive_planning: false` を設定すると、その動作を無効にします。planning prompt に対象 plan file path を埋め込み、agent に `plan.md` を直接書かせます。これは `command` backend と同じ方式です。完了後に `plan.md` を読み込み、file が作成されていなければ agent が捕捉した output を fallback として使います。

```yaml
interactive_planning: false   # tool-less, file-based planning
```

`--grill` には interactive planning flow が必要であり、`interactive_planning` が無効なら拒否されます。常に file-based の `command` mode ではこの field の効果はありません。

<a id="sdk-opencode---the-opencode-v2-client"></a>
### `sdk: opencode` — OpenCode V2 client

OpenCode CLI は `cruise install opencode` で install するか別途 install し、認証してください。

```yaml
sdk: opencode
model: "provider/model#variant" # Replace with a model in your OpenCode setup
plan_model: "provider/planning-model"
interactive_planning: true
permission: full
steps:
  implement:
    prompt: "{input}"
```

workflow、planning、step の model には `provider/model[:effort]` または native `provider/model#variant` を使います。effort は `low`、`medium`、`high`、`xhigh`、`max` を指定できます。通常の effort と native `#variant` は併用できません。

model を省略すると OpenCode の設定済み default に委ねます。workflow と step の `env:` 値は継承 OS 環境とともに prompt tool に渡されます。既存の OpenCode global/project 設定と認証を引き続き使います。

Cruise の planning、質問、title 生成、PR metadata は MCP server ではなく、native V2 plugin を通じて既存 tool を使います。`computer_use: true` は明示的に非対応です。`permission: edit` は直接の file 編集を許可しますが、shell、subagent、未知の実行 tool を拒否します。`read-only` は変更も拒否しつつ、安全な Cruise tool と workspace snapshot check は維持します。制限 mode では未知の MCP と Code Mode execution 経路も拒否されます。これらは tool 制限であり OS sandbox ではありません。

workflow MCP entry はローカル `stdio` とリモート Streamable HTTP (`type: http`) に対応します。旧 `type: sse` は server 名を示す diagnostic で拒否され、暗黙に変換されません。無人 run 中に OAuth login は開始しません。workflow entry は source file を変更せず OpenCode 設定に重ねて適用します。

[OpenCode workflow の例](../../skills/cruise-config/examples/opencode-flow.yaml)を参照してください。

<a id="mcp-servers-mcp_servers"></a>
### MCP server (`mcp_servers`)

省略可能な top-level `mcp_servers` map は、すべての SDK prompt run に MCP server を提供します。workflow prompt step（parallel child を含む）、`after-pr` step、planning turn、session title 生成、PR-description 生成が対象です。`sdk: jcode`、適用される `default.yaml` layer を解決した後に `sdk` も `command` も残らない場合だけ使われる既定 `jcode` backend、`sdk: claude`、`sdk: opencode` に対応します。step-level override はありませんが、prompt の `disallowed_tools: [mcp]` または `["mcp:<name>"]` でその step から workflow entry を除外できます。user-level MCP 設定には影響しません。呼び出された workflow の top-level `mcp_servers` は無視され、caller の設定が維持されます。

entry は Claude Code `mcpServers` の server 形式です。ローカル server では `type` を省略するか `type: stdio` を指定します。SDK 指定 object と authoritative retry-chain entry を含む、適用されるすべての有効 candidate が transport に対応する必要があります。stdio はすべての SDK に対応し、HTTP は Claude または OpenCode、旧 SSE は Claude のみ対応です。workflow SDK を変更しても SDK 指定 object は上書きされません。command backend には MCP server を渡せません。

```yaml
sdk: claude
mcp_servers:
  local_search:
    command: npx
    args: ["-y", "@example/search"]
    env:
      SEARCH_ROOT: ./docs
  remote_docs:
    type: http
    url: https://mcp.example.test/server
    headers:
      Authorization: "Bearer replace-with-your-token"
```

MCP entry の値は Cruise の `{variable}` template 解決を受けず、そのまま渡されます。名前は空でない必要があり、ASCII 英数字、`_`、`-` のみ使用できます。`cruise` は予約済みです。stdio entry は空でない `command` が必要で、`url` や空でない `headers` は指定できません。`http` と `sse` entry は `http://` または `https://` で始まる URL が必要で、`command` や空でない `args` / `env` は指定できません。未知の entry field は拒否されます。`command` backend に空でない `mcp_servers` map がある場合、検証エラーです。
