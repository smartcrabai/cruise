[English](../en/sdk.md) · [日本語](../ja/sdk.md) · [简体中文](../zh/sdk.md) · [目次](README.md)

# SDK backend

<a id="sdk-mode"></a>
## SDK mode

外部 CLI を `command` で起動する代わりに、top-level の `sdk` field を設定して SDK backend で prompt step を実行できます。`command` と `sdk` は相互排他です。適用される `default.yaml` layer を解決した後に `command` も `sdk` も残っていない場合に限り、既定の `jcode` backend が使われます。受け付ける値は `jcode`、`claude`、`opencode` の3つです。

<a id="startup-oauth-expiration-checks"></a>
### 起動時の OAuth 有効期限チェック

`cruise plan`、`cruise run`、`cruise exec` で agent を起動する前、および TUI/WebUI で session の planning/execution を始める前に、Cruise は該当する SDK 候補に限ってローカル OAuth credential file を調べます。planning では有効な `plan_model` を使い、なければ `model` を使います。execution では workflow model と step override（`after-pr`、group、parallel child を含む）を調べます。model array、SDK を指定した entry、有効な retry fallback chain も、実際には使われない fallback を含めて対象です。TUI/WebUI 起動時の全体検査ではありません。command backend、dry-run、`cruise workflow generate`、`cruise workflow make` は検査対象外です。

- **Claude SDK:** `$CLAUDE_CONFIG_DIR/.credentials.json` を読みます。未設定なら `~/.claude/.credentials.json` を使い、`claudeAiOauth.expiresAt`、`refreshToken`、省略可能な `refreshTokenExpiresAt` を参照します。
- **Jcode SDK:** source home の `auth.json` 内にある明示 provider `claude`、`claude-oauth`、`claude-subprocess` と、`openai-auth.json` 内の `openai`、`openai-oauth` だけを確認します。source home は process `JCODE_HOME`、未設定なら `~/.jcode` です。複数 account がある場合、すべての account が確実に期限切れと判断されたときに限り停止します。
- **スキップ対象:** OpenCode、provider を含まない jcode bare model、`anthropic-api` / `openai-api` などの API key provider、その他すべての provider。file の欠落／読み込み不可、不正 JSON、有効期限データ不足の場合はブロックせず通過します。Claude では process または workflow-level `env` に `ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN`、`CLAUDE_CODE_OAUTH_TOKEN`、`CLAUDE_CODE_USE_BEDROCK`、`CLAUDE_CODE_USE_VERTEX`、`CLAUDE_CODE_USE_FOUNDRY` のいずれかに空でない値があれば file 検査をスキップします。

例外として、foreground CLI の承認時に行う SDK title 生成（`cruise plan --skip-planning` を含む）は、継承した process 環境と process の working directory のみを使います。この有効期限事前検査では `CLAUDE_CONFIG_DIR` や別認証方式の環境変数を含め、workflow-level `env` を無視します。workflow のみで設定された API key は title 生成の検査を回避しません。継承した OAuth credential が確実に期限切れなら承認を停止します。

access token が期限切れで、refresh token がないか、記録された refresh token の有効期限が現在時刻以前の場合に限り Cruise は停止します。access token が期限切れでも refresh token があり、その有効期限が不明なら警告せず通過し、backend の自動 refresh に委ねます。この検査は file の読み取りだけで、auth-status CLI を呼び出さず、provider に接続せず、token を refresh／書き込みせず、credential lock も取得しません。再ログインには必要に応じて `claude auth login`、`jcode login claude`、`jcode login openai` を使います。

これは有効期限の事前検査であり、認証成功の証明ではありません。refresh token の server 側失効を検出できず、jcode の refresh-failure state や macOS Keychain credential も読みません。step-level `env` の API key は考慮されません。誤検知を回避するには process 環境変数 `CRUISE_SKIP_AGENT_AUTH_CHECK=1` を設定してください。正確な値 `1` のみが回避し、`true` や空白付き値では回避されません。

```yaml
sdk: jcode        # Explicitly select jcode. If omitted, it is the fallback only
                  # when neither `command` nor `sdk` remains set in the resolved
                  # workflow after applicable `default.yaml` layers are applied.

# Every actual jcode prompt needs an effective model. Use
# `provider/model[:effort]` or a bare model available in your jcode setup.
# model: provider/model[:effort]
```

すべての SDK backend で `model` / `plan_model` / step-level `model` は **model reference** であり、command mode と同じ override 優先順位（step `model` > top-level `model` / `plan_model`）が適用されます。jcode と Claude では、省略可能な `:effort` suffix が推論 effort tier を選択します。`low` / `medium` / `high` / `xhigh` / `max` と alias `minimal` / `min` / `med`、数値表記 `1`..`4` が使えます。`off` / `none` / `0` / `5` も消費されますが effort は未設定のままです。それ以外の `:` suffix（例: OpenRouter の `:free` variant）は model ID の一部として残ります。jcode/Claude では provider または model が未対応の effort は無視されます。OpenCode は catalog に照らして effort を検証し、非対応なら明示的に失敗します（後述）。

workflow-level `model` と `plan_model` は array にすることもできます。SDK mode では最初の entry が primary model、後続 entry が暗黙の fallback chain になります。fallback entry を持つ model array は自動的に model fallback を有効にします。primary model に対する明示的な `retry.fallback_chains` entry は array の残りより優先されます。429、5xx、429 以外の HTTP 4xx、401、403、407、408、network failure の場合、`--rate-limit-retries` が0より大きく、表示済みテキストがなければ、利用可能な fallback に即座に切り替えます（利用可能な fallback がない場合だけ429を同じ model で再試行します）。同じ 4xx request は同じ失敗を招くため、同じ model に再送されません。401、403、407 は他の model では解決できない認証／permission error なので、chain を進まず直ちに step を失敗させます。408 は network failure として扱われるため、5xx と同様に利用可能な fallback に先に切り替えます。同じ model で再送するのは fallback がない、表示済みテキストがある、または `--rate-limit-retries` が `0` の場合だけです。429、5xx、network failure（408 を含む）で離れた model と、provider が不在と報告した model (`400 model_not_supported`、`404 model_not_found`) は、その process 内で30分 cooldown されます。原因が model ではなく request である通常の client error で離れた model は cooldown されません。command mode では最初の entry のみを使い、従来の同一 model retry 動作を維持します。

対応する model reference:

- `"provider/model[:effort]"`（例: `provider/model:xhigh`）— `sdk: jcode` は provider と model を分けて parse します。`"/model"` や `"provider/"` のように片側が空なら prompt 実行時に失敗します。`sdk: claude` は `:effort` suffix 以外すべてを model 名として扱います。
- `"model"`（`/` なし）— jcode/Claude では明示 provider を指定しません。OpenCode の設定済み reference には provider が必要です。
- `"provider/model[:effort]"` または `"provider/model#variant"` — `sdk: opencode` 用です。model ID 内の `/` と `:free`、`:2026` のような未知の colon suffix はそのまま残ります。認識される末尾 effort suffix は予約済みで、`#variant` と組み合わせられません。
- 未設定 — 明示 model を渡しません。OpenCode は設定済み default を使います。有効 model が依然として存在しない jcode prompt は setup error で失敗します。Claude と `command:` は従来どおり model なしで動作します。

<a id="subagent-model"></a>
### subagent model

workflow-level または prompt step に `subagent_model` を設定して、spawn する agent の model を選びます。step 値は workflow 値を上書きします。値は空白でない単一文字列、または `{sdk: claude, model: haiku}`（`sdk: jcode` も可）です。array や fallback chain は受け付けません。文字列は workflow SDK を継承し、既定は jcode です。fallback 試行中の SDK は継承しません。解決済み subagent SDK と一致する SDK の試行にのみ設定が渡り、その他の試行は backend default を使います。適用できる model candidate がない設定は検証時に拒否されます。

```yaml
sdk: claude
model: sonnet
subagent_model: haiku
steps:
  review:
    prompt: Review the implementation, delegating checks as needed.
    subagent_model: {sdk: claude, model: sonnet}
```

- Claude は `CLAUDE_CODE_SUBAGENT_MODEL` と `CLAUDE_CODE_SUBAGENT_MODEL_FORCE=1` を設定し、Explore、Plan を含むすべての subagent を固定します。これらの注入値は同名の workflow/step `env:` key を上書きします。`subagent_model` がなければ、それらの環境 key は変更しません。
- jcode は prompt の前に session の swarm default を設定します。worker が明示的に指定した `model` は引き続き優先されるため、これは強制ではなく default override です。
- OpenCode の subagent model と `command:` backend は非対応です。mixed-SDK workflow では明示的な SDK object で Claude/jcode candidate を対象にできます。認識される `:effort` suffix は `:off` も含め拒否されます。`:free` など未知の suffix は model ID の一部です。jcode では有効な空でない model が必要です。
- group、`after-pr`、parallel prompt child、`github-review` を含む workflow prompt step、および planning の plan/fix/ask turn に適用されます。title 生成、PR metadata、`workflow generate`、`workflow make` には適用されません。prompt 以外の step と group/workflow-call/parallel wrapper には設定できません。
- 呼び出された workflow の top-level 値は caller の default に置き換えられます。callee 内の step override は保持されます。
- レイヤー化された `default.yaml` と `CRUISE_SUBAGENT_MODEL` は対応しますが、アプリケーション `config.json` / `config.yaml` の default は対応しません。

<a id="sdk-jcode--the-jcode-sdk-default"></a>
### `sdk: jcode` — jcode SDK（既定）

`sdk: jcode` は公式 jcode Rust SDK を使い、jcode v0.88.0 以降を必要とします。起動時に Cruise は SDK handshake と必須 capability（`sessions`、独自 tool が登録される場合は `session_tools` も）を検証しますが、CLI version は調べません。必須 capability がない場合は jcode を upgrade してください。`model`、`plan_model`、step-level `model` field には `provider/model[:effort]` または bare model を指定します。実際に実行するすべての jcode prompt で model が解決されている必要があります。通常の prompt は step `model`、次に workflow/default `model` を使います。planning call は `plan_model`、次に `model` を使います。jcode に設定された default、`JCODE_MODEL`、または resume session は Cruise 側で model が未設定の場合の代わりになりません。model がなければ user `default.yaml` template を作成または保持し、その要求を停止します。effort suffix は SDK 経由で適用されます。

各 SDK run は継承した login record と model catalog のコピーを持つ private session home を使います。runtime の `JCODE_HOME` は実際の session directory を指し、Unix では SDK socket path 専用の短い symlink alias を使います。これにより保存 session の場所や再開方法を変えずに、jcode の account-store 安全性チェックを満たします。

<a id="computer-use"></a>
#### Computer use

workflow-level `computer_use` の既定値は `false` です。`true` にすると macOS 上の prompt turn で jcode の `macos_computer_use` desktop-control tool を許可します。prompt step では `computer_use: true` または `false` を指定して workflow default を上書きできます。組み込み plan、fix-plan、ask-plan turn は workflow 値に従い、title と PR-description 生成では常に computer use を無効にします。この tool は macOS 専用で、Linux では設定の効果はありません。SDK を指定した object や authoritative retry chain で選択される候補を含め、適用可能な有効 model candidate はすべて jcode を使う必要があり、`command:` backend は設定できません。明示的な jcode-only candidate は Claude または OpenCode workflow SDK の下でも使用できます。workflow SDK だけを変更しても、互換性のない SDK 指定 object は上書きされません。

computer use が無効な場合、Cruise は workflow `env:`、Cruise process 環境、private copy の `config.toml` 内 `[tools].disabled` の順に最初に見つかった list とマージして、`JCODE_DISABLED_TOOLS` に `macos_computer_use` を追加します。これは swarm worker と subagent を含む private runtime の daemon に届きます。computer use が有効で permission が full、明示的な web/subagent 拒否がない場合、Cruise は `JCODE_DISABLED_TOOLS` を変更せず、jcode 側で無効な tool を強制的に有効化しません。ユーザーには macOS の Accessibility と Screen Recording permission が必要です。

<a id="permission-modes"></a>
### permission mode

workflow-level `permission` の既定値は `full` です。prompt step に `permission: read-only | edit | full` を設定して上書きできます。`full` は現在の動作を維持します。`edit` は shell 実行（Claude `Bash`、jcode `bash`）を無効にしますが、直接の edit/write/patch tool は維持します。`read-only` はさらに file 変更 tool（Claude `Edit`/`Write`/`NotebookEdit`、jcode `edit`/`write`/`apply_patch`/`replace`）を無効にします。制限 mode では `computer_use: true` でも `macos_computer_use` を強制的に無効化します。Claude は無人実行を permission prompt で停止させないため `dontAsk` で実行します。jcode の mode 固有 tool は選択済み `JCODE_DISABLED_TOOLS` list にマージされます。

- Cruise は任意の CLI tool を制御できないため、`command:` backend では `read-only` と `edit` を拒否します。
- `read-only` と `allow_commit: true` の併用は設定エラーです。`allow_commit: true` は HEAD/ref commit guard と暗黙の Git 拒否だけを解除します。明示的制限は解除せず、permission mode を広げません。
- group call、`workflow_call`、parallel wrapper には `permission` を設定できません。内側の prompt または parallel prompt child に設定してください。read-only child が1つでもある parallel block は、read-only prompt child のみで構成する必要があります。
- `workflow_call` の callee top-level `permission` は無視され、caller の default が適用されます。callee 内 step-level override は保持されます。
- read-only step は step 実行中に workspace snapshot が変わると `ReadOnlyWorkspaceChanged` で失敗します。これは変更検出であり、OS sandbox や rollback ではありません。`.git`、`target`、`node_modules` は対象外です。
- 組み込み planning、fix/ask、title、PR-description、workflow-generation turn は `full` で実行され、`disallowed_tools` や暗黙の Git 制限は適用されません。

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

backend ごとの強制方法と制約:

- **Claude:** native deny rule は `web` を `WebFetch`/`WebSearch`、`subagent` を `Agent`、shell prefix を `Bash(...)` に対応付けます。`bypassPermissions` でも deny は有効です。Claude native matching は Git global option を**正規化しない**ため、`git -C path commit` は `git commit` に一致しない場合があります。
- **jcode:** user entry を保持しながら `webfetch`、`websearch`、`browser`、`swarm` を `JCODE_DISABLED_TOOLS` にマージします。shell rule には Cruise の pre-tool gate を使い、既存 hook の末尾に追加します。workflow env、process env、config `[hooks].pre_tool` の順で選択します。workflow または process env の `JCODE_HOOKS_DISABLED` は gate を無効にして警告を出します。gate setup の失敗も警告になります。不正な policy/input を含め block する場合、gate は exit code `2` を返します。hook 実行失敗やそれ以外の exit code は jcode で fail-open です。既存 hook と組み合わせるのに必要な array 形式は、古い release ではサポートされない場合があります。
- **OpenCode:** native deny rule に加えて Cruise plugin/bridge が web tool、`task` delegation、一致する bash input の実行前拒否を行います。
- **jcode/OpenCode の shell matching:** 複合 command を分割し、先頭の代入と共通 wrapper (`env`、`command`、`builtin`、`exec`、`nohup`、`nice`、`time`、`timeout`、`stdbuf`、`xargs`) を除去し、`sh`/`bash`/`zsh`/`dash` の `-c` script を調べ、実行 file の basename を比較し、Git global option を正規化します。heredoc 本文によって誤検知の拒否が起きる場合があります。完全な shell interpreter でも security boundary でもありません。

MCP 制限が filter するのは backend 起動前の workflow 提供 entry のみです。user-level backend MCP 設定と Cruise 独自 session tool は有効なままで影響を受けません。これらの制御は多層防御であり sandbox ではありません。prompt warning、reference-transaction hook、CAS rollback も引き続き適用されます。

**更新に関する注意:** macOS では以前の Cruise version はすべての jcode prompt に暗黙で `macos_computer_use` を公開していました。その動作を維持するには workflow-level `computer_use: true` を設定してください。

GitHub Action の shell bootstrap は `jcode version --json`、`jcode login`、`jcode provider add`、観測目的の `jcode auth status` を引き続き使います。Action に SDK client はなく、SDK からは CLI version string や account/profile setup 操作を利用できないためです。Cruise backend 自身は起動後に SDK capability handshake を使います。

Cruise は process `JCODE_HOME` を source home として解決し、未設定なら `~/.jcode` を使います。各 Cruise session は credential と config を継承する分離 SDK home を持ち、再開した plan turn ではそれを再利用します。private runtime には `JCODE_CHECK_UPDATES=0` を渡しますが、コピー済み config は書き換えません。plan home は Cruise session が削除／clean されるまで保持されます。再開しない prompt home は各 turn 後に削除されます。使用中の run がないことを cross-process lock で確認できる場合に限り、未使用 home は24時間後に prune されます。private home の削除前に Cruise が停止するのは、command と `JCODE_HOME`/`JCODE_RUNTIME_DIR`/`JCODE_SOCKET` がその runtime を特定する生存中の `jcode serve` process だけです。registry PID だけを根拠にはしません。別 directory から Cruise を起動しても削除できるよう、resumable plan session とともに絶対 source-home path を保存します。

model catalog cache は private home にコピーされます。path-bound external credential は link ではなく owner-only file としてコピーされ、元の path に既に記録された trust decision のみを継承します。

`ask_user` は jcode の SDK callback deadline 120秒を超えても保留中の質問を維持します。約110秒回答がないと、tool は成功の continuation notice を返し、model に同一質問で `ask_user` を再度呼び出すよう求めます。ユーザーに再度質問することはなく、呼び出し間に届いた回答は次の呼び出しで返します。待機時間は workflow step timeout の範囲内です。保存済み plan conversation が private jcode home にない場合、現在の plan が既に prompt に含まれているため Cruise は新しい SDK session を開始します。

`submit_plan` と `update_plan` は plan を書き込んで返るだけです。Cruise の既存承認フローは model turn の後に行われ、SDK tool callback を開いたままにしません。

**更新に関する注意:** MCP-bridge release から upgrade した後は、source `$JCODE_HOME/mcp.json`（`JCODE_HOME` 未設定なら `~/.jcode/mcp.json`）に残る古い `mcpServers.cruise` entry を削除してください。Cruise は各 private session copy からこの entry を除去しますが、source config は変更しません。削除するまで Cruise 外の jcode session は引き続き起動します。この entry が残ると、Cruise は従来の plan input として扱わず stderr に移行手順を出して終了するため、jcode では `cruise` MCP server が失敗として報告されます。

backend は jcode の session-tools support を通じて Cruise session tool を公開します。

<a id="sdk-claude--the-claude-cli-in-process"></a>
### `sdk: claude` — in-process の claude CLI

`sdk: claude` は claude-agent-sdk を通じて `claude` CLI を in-process で実行します。model reference は通常の `claude --model` 名で、省略可能な `:effort` suffix は `--effort` として渡されます。この flag に対応しない `claude` CLI では step が `unknown option '--effort'` で失敗します。この error は permanent と分類され、retry されません。Cruise は 2.1.250 で検証されています。認証には claude CLI 固有の保存済み credential または `ANTHROPIC_API_KEY` を使い、`jcode login` とは無関係です。Cruise workflow は無人実行のため CLI は permission bypass 付きで動作します。そのため permission prompt に応答する console はありません。Cruise の workflow tool は SDK 経由で公開されます。Claude Code の computer use は interactive session でしか使えず Cruise から有効化できないため、`computer_use: true` は config load 時に拒否されます。

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

Cruise には workflow が必要とする OpenCode V2 API の Rust client が含まれます。Python/Node SDK dependency や provider 固有 LLM client ではありません。OpenCode CLI は `cruise install opencode` で明示的に install するか別途 install し、認証してください。Cruise は短時間だけ loopback の `opencode serve --stdio` process を所有し、各試行後に停止します。workflow execution では外部 server への接続、CLI の install、代理認証、V1 への fallback は行いません。

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

workflow、planning、step の model（array candidate と再開 session を含む）には `provider/model[:effort]` または native `provider/model#variant` を使います。一般的な effort 名と alias は大文字小文字を区別しません。`low` (`min`/`minimal`/`1`)、`medium` (`med`/`2`)、`high` (`3`)、`xhigh` (`4`)、`max`) を使用できます。Cruise は要求された tier を OpenCode の live model catalog で検証して対応 variant を選び、推測や downgrade はしません。未対応／曖昧な effort、catalog にない model、tier を持たない `:off`/`:none`/`:0`/`:5` suffix は、prompt 送信や再開 model 変更の前に明示的に失敗し、fallback しません。通常の effort と native `#variant` は併用できません。`#none` を含む native variant、suffix なし model、model 省略は従来の動作を維持します。

model array は既存の failure-driven fallback policy を使います。OpenCode の本物の `provider.no-route` execution failure（`Model unavailable` を含む）は missing model として扱い、設定済みで利用可能な fallback に既存 budget と cooldown を適用しながら直ちに切り替えます。一般的な unavailable 文字列、その他の failure kind、追記された stderr、認証／permission error、不正 request はこの特別処理の対象外です。表示済み output と replay barrier があれば切替は抑止されます。tool の副作用、prompt 受理が不確か、または event を復旧できない場合、表示 text がなくても OpenCode は自動 retry/fallback を禁止します。model を省略すると OpenCode の設定済み default に委ねます。workflow と step の `env:` 値は継承 OS 環境、commit guard とともに prompt tool に渡されます。既存の OpenCode global/project 設定と認証を引き続き使います。Cruise は独自 database、plugin、再開可能 plan conversation を project やユーザーの OpenCode database ではなく private XDG storage に保管します。ユーザー設定は書き換えません。不正な `OPENCODE_CONFIG_CONTENT` は破棄せず診断します。

Cruise の planning、質問、title 生成、PR metadata は MCP server ではなく、native V2 plugin を通じて既存 tool を使います。`computer_use: true` は明示的に非対応です。`permission: edit` は直接の file 編集を許可しますが、shell、subagent、未知の実行 tool を拒否します。`read-only` は変更も拒否しつつ、安全な Cruise tool と workspace snapshot check は維持します。制限 mode では未知の MCP と Code Mode execution 経路も拒否されます。これらは tool 制限であり OS sandbox ではありません。

workflow MCP entry はローカル `stdio` とリモート Streamable HTTP (`type: http`) に対応します。旧 `type: sse` は server 名を示す diagnostic で拒否され、暗黙に変換されません。無人 run 中に OAuth login は開始しません。workflow entry は source file を変更せず OpenCode 設定に重ねて適用します。

protocol の基準は OpenCode commit `74f8d4860f49c563909e6b039b0cc8b454ebce72`（2026年10月23日調査）です。OpenCode v2.1.3 は最初の実 CLI 検証対象として特定されたもので、全般的な互換保証ではありません。runtime V2 capability check と fixture test だけでは、実 model や plugin の相互運用性は立証されません。

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

jcode の場合、Cruise は workflow entry を `$JCODE_HOME/mcp.json`（`JCODE_HOME` 未設定なら `~/.jcode/mcp.json`）の private session copy にマージします。同名の workflow entry は source file の同名 entry を置換しますが、source は変更されません。その後 jcode は通常の設定マージ順を適用するため、`~/.claude.json`、`~/.claude/mcp.json`、project-local `.jcode/mcp.json`、`.mcp.json`、`.claude/mcp.json` にある同名 server は workflow copy より優先されます。jcode は workflow-level `env:` を含む runtime 環境を使って `${VAR}` と `${VAR:-default}` を展開します。

`sdk: claude` では workflow MCP entry を Unix で mode `0600`（Windows では owner-only ACL）にした run 専用 private JSON file に書き込み、file path を `--mcp-config` で渡します。SDK は Cruise の in-process tool を別途、自身の inline `--mcp-config` 値として渡します。ユーザー自身の Claude MCP 設定も有効なままです。Claude entry の `${VAR}` 展開は保証されません。
