[English](../en/steps.md) · [日本語](../ja/steps.md) · [简体中文](../zh/steps.md) · [目次](README.md)

# Step リファレンス

<a id="step-types"></a>
## Step の種類

<a id="prompt-step-llm-call"></a>
### Prompt step（LLM 呼び出し）

prompt step には、成功した試行後に次へ進む前の検証に使う `quality_gates` を宣言できます。[Quality gate](workflow-control.md#quality-gates-quality_gates)を参照してください。

```yaml
steps:
  planning:
    model: claude-opus-4-5        # model to use (optional; overrides top-level model)
    instruction: |                # message displayed before the step runs (optional)
      Before planning, add any constraints or context:
    prompt: |                     # prompt body (use either prompt or prompt_file)
      Create an implementation plan for:
      {input}
    timeout: 10m                  # per-step timeout (optional; see Step Timeout)
    output_file: initial-state.md # save the final response as a session artifact (optional)
    allow_commit: false           # default: guard this prompt from moving Git HEAD in this repository
    computer_use: true            # optional; jcode only; overrides top-level computer_use
    env:                          # environment variables for this step (optional)
      ANTHROPIC_MODEL: claude-opus-4-5
```

`output_file` を設定すると、cruise は完了した backend response を workspace の外にある session の `artifacts/` directory に保存します。後続 prompt body から `{file:initial-state.md}` で読み取れます。

`instruction` は step 実行直前にユーザーへ表示する message であり、system prompt ではありません。`{input}` が空の場合、message は複数行入力 prompt として使われ、送信されたテキストが `{input}` になります。それ以外の場合は表示されるだけで LLM には送られません。

prompt step は既定で commit guard の対象です。SDK prompt には [`tool restrictions`](sdk.md#tool-restrictions) で説明する暗黙の Git shell 拒否も適用されます。`allow_commit: true` はこれらと guard を解除しますが、明示的な `disallowed_tools` は解除せず、権限を広げません。prompt が意図的に commit を作成する、または Git `HEAD` を動かす必要がある場合に限り `allow_commit: true` を設定してください。省略時および `false` では guard が有効です。guard は従来の `command:` mode と `sdk: jcode` を対象とします。command/option step には影響しません。

`allow_commit: true` は `group:` や `workflow_call:` 呼び出しではなく、実際の prompt に設定してください。

長い prompt は `prompt_file` で file から読み込めます。

```yaml
steps:
  implement:
    prompt_file: prompts/implement.md
```

解決規則:

| `prompt_file` の値 | 解決方法 |
| --- | --- |
| 絶対 path | そのまま使用 |
| `~/...`、`~`、`~user/...` | 現在の user または指定 user の home directory に展開 |
| `./...`、`../...`、bare file 名 | 設定 file を含む directory からの相対 path |
| GitHub blob/raw URL | GitHub から取得 |
| 呼び出された workflow 内の相対 path | 呼び出された workflow の directory（または remote directory）からの相対 path |

絶対 path と `~` path はローカル filesystem を参照します。GitHub-hosted workflow 内の URL ではない相対値は remote directory 内の path として解決され、ローカル `~` path は拒否されます。直接 GitHub blob/raw URL を明示的に指定できます。

<a id="command-step-shell-execution"></a>
### Command step（shell 実行）

```yaml
steps:
  run_tests:
    command: cargo test           # single command (required)
    timeout: 5m                   # per-step timeout (optional; see Step Timeout)
    env:                          # environment variables for this step (optional)
      RUST_LOG: debug

  lint_and_test:
    command:                      # list of commands: run sequentially, stop on first failure
      - cargo fmt --all
      - cargo clippy -- -D warnings
      - cargo test
```

`accepted_exit_codes` を指定すると通常の command step が必須になります（`parallel:` 内の child は対象外）。リストには `0` が必要で、重複は不可。許可された非ゼロ終了では `{prev.success}` は `false` のままです。未許可の終了コード、シグナル、タイムアウト、起動失敗では `if.fail` に進まず workflow を止めます。省略すると通常の失敗経路を維持します。

<a id="parallel-steps"></a>
### Parallel step

`parallel:` を使うと、1つの session 内で名前付き prompt または command step を並行実行できます。すべての child が完了してから次の step が始まります。

```yaml
steps:
  checks:
    parallel:
      lint:
        command: cargo clippy -- -D warnings
      tests:
        command: cargo test
      review:
        prompt: Review the changes and report findings without editing files.
        timeout: 10m
    timeout: 15m
  summarize:
    prompt: "Summarize the check results: {prev.output}"
```

- すべての child は session の既存 working directory で同時に開始します。互いに独立した command または file 編集を使ってください。cruise は child を分離したり変更を merge したりしません。同じ file や Git state を同時に編集すると競合する場合があります。
- 各 child は block 開始前の同じ input と `{prev.*}` 値を受け取ります。環境の優先順位は workflow < parallel block < child です。child には `model`、`env`、`skip`、`when`、`timeout` を指定できます。prompt child のみ `instruction`、`computer_use`、`permission`、`disallowed_tools`（parent には設定不可）、`output_file` を指定できます。parallel の `instruction` は非対話であり、実行時 input が空でない必要があります。input を指定するか、その前に順次実行する instruction step を置いてください。そうしないと入力 editor を開かずに実行エラーになります。command array は child ごとに順次実行されます。
- join 後の `{prev.output}` は YAML 宣言順で child 名を key にした JSON object です。各 entry は `output`（prompt text。command は `null`）、`stderr`（実行 error を含む）、`success`、`skipped` を持ちます。すべての child が成功または skip の場合 `{prev.success}` は true です。`{prev.stderr}` は child 名とともに error を連結します。
- child の失敗で sibling は取り消されません。通常 step の failure semantics に従い、block 全体が1 step、最大1 failure として扱われます。parent に `if.fail` または `next` を設定してください。retry では block 全体を再実行します。
- child timeout の影響はその child のみに及びます。parent timeout は未完了 child を cancel し、failure 経路へ進む前に停止を待ちます。
- parallel block は `groups`、`after-pr`、呼び出された workflow 内でも使えます。child は prompt/command step に限られます。child の `next`、`if`、`option`、`plan`、`group`、`workflow_call`、nested `parallel`、`flag`、`allow_commit: true` は拒否されます。`flag` を含む flow control は parent に設定し、commit はその後の順次 step で行います。

<a id="step-timeout"></a>
### Step timeout

prompt、command、parallel step には、実行時間が長すぎる場合に中止する `timeout:` を設定できます（option step は無視します。command array では command ごと、parallel block では block 全体に適用）。形式:

| suffix | 意味 | 例 |
|--------|---------|---------|
| （なし） | 秒 | `timeout: "30"` |
| `m` | 分 | `timeout: 5m` |
| `h` | 時間 | `timeout: 1h` |

timeout が発生した場合:

- **Command step:** child process を終了し、`{prev.success}` を `false` にします。`accepted_exit_codes` を指定した command は workflow を止め、それ以外は通常の失敗経路（`if.fail`）に進みます。
- **Prompt step:** LLM 呼び出しを中断し、step を失敗として扱います。

不正な timeout 文字列は設定検証時に拒否されます。group と `after-pr` 内で定義された step にも timeout は適用されます。

<a id="option-step-interactive-selection"></a>
### Option step（対話選択）

`option` の各 item は `selector`（menu 選択肢）または `text-input`（自由記述 prompt）です。省略可能な `plan` field は file path に解決され、その内容を menu 表示前に枠付き panel で表示します。

```yaml
steps:
  review_plan:
    plan: "{plan}"               # optional: display contents of this file before the menu
    option:
      - selector: Approve and continue   # shown in selection menu
        next: implement
      - selector: Revise the plan
        next: planning
      - text-input: Other (free text)    # shows a text prompt when selected;
        next: planning                   # entered text is available as {prev.input}
      - selector: Cancel
        next: ~                          # null/omitted next falls through to the
                                         # next declared step; the last step ends the workflow
```

<a id="post-pr-automation-after-pr"></a>
## PR 後の自動処理 (`after-pr`)

ホストが作成する commit と PR body の整形については、[commit と PR の template](configuration.md#commit-and-pr-templates)を参照してください。

`after-pr` は、`cruise run` が pull request の作成に成功した後で自動実行する step に使います。`steps` と同じ形式なので、inline または file-backed prompt step (`prompt` / `prompt_file`)、command step、group step を定義できます。

```yaml
steps:
  implement:
    prompt: "{input}"

  test:
    command: cargo test

after-pr:
  notify:
    command: "echo 'PR #{pr.number} created: {pr.url}'"

  label:
    command: "gh pr edit {pr.number} --add-label enhancement"
```

`after-pr` step は PR 作成成功後に限り実行されます。通常の template 変数すべてと、下記の PR 専用変数を使えます。

`after-pr` の prompt step にも同じ既定の commit guard が適用されます。HEAD を進める必要がある after-PR prompt には個別に `allow_commit: true` を設定してください。

組み込みの PR 後 prompt には commit、push、forge 操作を禁止します。ホストが検証した競合解消を commit し、対象 PR/MR の branch にだけ push します。CI 修復は現在の HEAD の証拠に基づき、ホスト verifier が実行可能な場合は正確な stage 済み修復を公開前に検証します。実行できない場合は Cruise が warning を表示し、ホスト検証なしで公開して push 先 HEAD の CI に依拠します。その後 CI を再確認し、session 内で同じ HEAD の対象ジョブを一度だけ再試行するか、マージせず停止します。停止した session を再実行すると、前回の判定を使い回さず現在の HEAD の CI を再確認します。native merge は準備状態を独立に確認します。GitHub で、check が queued や実行中のまま（runner 待ちのジョブなど）`wait-ci` が55分の期限に達した場合も、native merge は待ち続けます。現在の HEAD の CI が pending のポーリングごとに10分の readiness 予算を数え直し、merge step の開始から最大60分まで延ばします。CI の失敗、conflict、レビューの要求ではこれまでどおりすぐに待ちを終え、それ以外の pending（mergeability が不明、CI 未登録）は10分の上限のままです。

競合解消を commit する前に、ホストは記録した競合 path 以外の tracked file の編集と、ignore されていない untracked path の追加・削除を拒否します。ignore された path は比較しません。ホストは競合 path だけを stage するため ignore された file が commit に入ることはなく、resolver の検証は `node_modules/.vitest-cache` や `tsconfig.tsbuildinfo` などの ignore された cache を作成・更新・削除して構いません。

`push-after-sync` が失敗した後に競合中の merge を中止した場合（`git merge --abort`）、session を再実行すると、記録した `MERGE_HEAD` を必要とする commit を再試行せず、after-PR を `sync-base` からやり直します。対象は、worktree が記録した PR branch の merge 前の HEAD にあり、進行中の merge、unmerged path、tracked file の変更、ignore されていない untracked path がない場合だけです。それ以外は今までどおり失敗します。新しい sync で競合を記録し直し、resolver を再び実行します。

GitHub で、失敗したチェックがすべて「runner が取得されなかったため GitHub が cancel した Actions ジョブ」の場合、組み込みの `rerun-unacquired-ci` step が失敗ジョブを再実行し（`gh run rerun --failed`）、修復 prompt を使わずに `wait-ci` へ戻ります。ホストは API でジョブごとに確認します: conclusion が `cancelled`、runner の割り当てなし、step なし、check run に failure annotation `The job was not acquired by Runner ...` があること。CI evidence には `Runner unacquired (host-verified)` として記録します。本物の失敗が混ざっている場合は今までどおり修復 prompt が扱い、自動再実行はしません。修復は新しい HEAD を push して CI を再実行させるうえ、`--failed` は本物の失敗も再実行してしまうためです。再実行は PR の HEAD ごとに最大 `ci_unacquired_rerun_max_attempts` 回（既定 `2`、`0` で無効）で、回数は session に保存されるため再開しても数え直しません。上限に達すると修復 prompt が evidence を受け取って `wait` を返し、上限を示すメッセージとともにマージせず停止します。この再実行も workflow の `max_retries` の edge budget を消費します。GitLab は変わりません。この step より前に作成した session は保存済みの after-PR step を使うため、自動再実行しません。

`require_bubblewrap: true` と daemon 環境変数 `CRUISE_JCODE_BUBBLEWRAP=1` は、利用可能な場合に Linux jcode Bubblewrap による隔離を要求します。Cruise は process ごとに一度、利用可否を probe します。`jcode-bubblewrap` feature がない build、Linux 以外の host、または probe の失敗時、step は隔離なしで実行され warning が表示されます。`command:` backend も拒否されず、隔離なしで実行され warning が表示されます。probe 成功後に Bubblewrap の起動が失敗した場合は attempt が失敗します。組み込みの競合解消・CI 修復・レビュー prompt は `require_bubblewrap: true` を維持するため、可能な場合に隔離されます。

CI 修復の自動 commit では、Bubblewrap が利用可能で `CRUISE_CI_VERIFY_SCRIPT` が設定されている場合、ホストが正確な stage 済み修復を commit 前に検証します。そうでなければ Cruise は warning を表示し、ホスト検証なしで commit / push して、push 先 HEAD の CI に依拠します。設定された verifier が失敗した場合は commit を停止します。`CRUISE_CI_VERIFY_SCRIPT` には daemon 環境から worktree 外にある信頼済み shell script の絶対 path を指定します。ハードリンク不可、group/other 書き込み不可。合否判定は候補ツリー内の script に任せません。script は読み取り専用の `/candidate` と書き込み可能な専用 `/tmp` を使い、ネットワークも継承資格情報も使えません。非対応ツリー（symlink や submodule など）は公開しません。保存済み session は元の workflow を保持します。

GitLab の merged-results pipeline が一時マージ SHA 上で動く場合は、再試行・自動修復を許可しません。source HEAD の証拠が必要です。

<a id="review-loop-github-review"></a>
### Review loop (`github-review`)

明示的な `after-pr` step で CodeRabbit など review bot を処理できます。汎用組み込み workflow はこれを追加しません。既存 PR では `builtin:pr-review-fix` が使用します。

```yaml
after-pr:
  coderabbit:
    github-review:
      bots: ["coderabbitai[bot]"]   # any one of these reviewing the current PR head ends the wait
      max-iterations: 3             # default 3
    prompt: |
      Fix the review comments without staging, committing, or pushing.
      Reply with the requested JSON describing each thread.
    allow_commit: true
    timeout: 30m                    # required; bounds the whole loop
    output_file: review-report.json # optional iteration report in the session artifacts
```

bot mode では各 iteration で、列挙した bot が現在の head commit を review するまで待ってから、その bot が開始した未解決 thread をすべての page から取得します。all-reviewer mode では新しい review を待たず、対象となる未解決 thread を直ちに取得します。
どちらの mode でも thread data を prompt に追加し、`{"actions":[{"thread_id":"...","reply":"optional","resolve":true}]}` の JSON のみを受け取ります。この step の `allow_commit: true` は、prompt ではなく Cruise に検証済みの変更の commit / push を許可します。公開に成功した後、または変更不要と確認した後に、認証済み `gh api graphql` で返信・解決します。失敗すると run は `Failed` になります。`after-pr` 直下でのみ有効です。

既定の `reviewers: bots` はこの動作を維持し、空でない一意な `bots` list が必要です。既存の human と bot の feedback を処理するには `github-review: { reviewers: all, max-iterations: 3 }` を使い、`bots` を省略するか空 list にします。この mode は新しい review を待ちません。Cruise は author の repository collaborator permission を確認し、`write`、`maintain`、`admin` の author の thread のみを対象にします。対象となる未解決 thread がなくなると loop は成功し、iteration limit 到達時に対象 thread が残る場合は失敗します。

<a id="variable-reference"></a>
## 変数リファレンス

commit と PR template は別の[許可済み語彙](configuration.md#commit-and-pr-templates)を使い、`{plan}` は含みません。以下の `{plan}` は引き続き session plan file path を意味します。

| 変数 | 説明 |
|----------|-------------|
| `{input}` | CLI argument または stdin からの初期 input。`--skip-planning` / input-as-plan session (`input_as_plan: true`) では現在の `plan.md` 内容に解決 |
| `{prev.output}` | 直前の prompt output、または child 名を key とする parallel block の JSON 結果 |
| `{prev.input}` | 直前の option step でユーザーが入力した text |
| `{prev.stderr}` | 直前の command/prompt の stderr、または child 名を prefix に付けた parallel block の stderr と実行 error の結合 |
| `{prev.success}` | 直前の command の成功状態、または parallel block の成功状態 (`true`/`false`。skip された child は成功として数える) |
| `{plan}` | session plan file path（`cruise run` が自動設定） |
| `{file:name}` | `name` という相対 session artifact の UTF-8 内容。prompt field のみで利用可能で、実行直前に読み込み |
| `{plan.language}` | 組み込み planning prompt で使う有効言語（`CRUISE_LANGUAGE_PLAN`、`languages.plan`、旧 field、locale 推定、default から決定） |
| `{pr.number}` | PR 作成後に利用できる pull request number |
| `{pr.url}` | PR 作成後に利用できる pull request URL |
| `{pr.language}` | PR title/body 生成で使う有効言語（`CRUISE_LANGUAGE_PR`、`languages.pr`、旧 field、locale 推定、default から決定） |
| `{cruise.bin}` | 実行中の Cruise 実行ファイルのパス。step の shell で 1 語として扱える形式（`sh` では single quote、Windows では空白を含まないパスで、必要なら 8.3 短縮名）。built-in step は `{cruise.bin} internal ...` を呼ぶため、`PATH` 上にある別バージョンの `cruise` を実行しません。この変数より前に保存された built-in の session snapshot も、読み込み時に `cruise internal ...` command を同じ扱いにします。ユーザーが書いた workflow は変更しません |

> **注意:** `{model}` は template 変数ではありません。top-level `command` array 内でのみ解決される特別な placeholder です。`prompt`、`prompt_file`、`instruction`、command step field 内では利用できません。

literal の波括弧は Rust `format!` 形式で escape します。`{{` → `{`、`}}` → `}` です（例: `"{{input}}"` は `input` の lookup ではなく literal string `"{input}"` に解決されます）。閉じていない `{`、単独の `}`、空の `{}`、未定義変数の参照は template 構文エラーです。
