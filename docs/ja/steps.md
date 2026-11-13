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

`output_file` を設定すると、cruise は完了した backend response を workspace の外にある session の `artifacts/` directory に保存します。file は atomic に書き込まれ、response の完全一致テキストを保持し、workspace 変更検出には含まれません。後続 prompt body から `{file:initial-state.md}` で読み取れます。`prompt_file` から読み込んだ prompt を含め、file reference は prompt 実行の直前に解決され、挿入されたテキストは再帰的に展開されません。artifact 名は相対パスで session directory 内に収まる必要があり、保存／読み込み上限は1 MiBです。空、絶対パス、`.` または `..` path component を含む名前は拒否されます。`reports/initial-state.md` のような subdirectory は使え、書き込み時に directory が作成されます。symbolic link はたどらず拒否します。session の `artifacts/` root や管理対象の親 directory を含め、artifact path 上のどこかに symlink があれば read/write の両方が失敗します。最終 path が既に存在して通常 file でない場合も同様です。

baseline と regression の比較 flow は [`examples/file-artifacts.yaml`](../../examples/file-artifacts.yaml) を参照してください。artifact は session に属し、session を削除すると消去されます。終了状態になった `cruise exec` session は完了後に cleanup されますが、中断された exec session は ID で再開でき、`cruise run` session は再開に備えて artifact を保持します。

`instruction` は step 実行直前にユーザーへ表示する message であり、system prompt ではありません。`{input}` が空の場合、message は複数行入力 prompt として使われ、送信されたテキストが `{input}` になります。それ以外の場合は表示されるだけで LLM には送られません。

prompt step は既定で commit guard の対象です。SDK prompt には [`tool restrictions`](sdk.md#tool-restrictions) で説明する暗黙の Git shell 拒否も適用されます。`allow_commit: true` はこれらと guard を解除しますが、明示的な `disallowed_tools` は解除せず、権限を広げません。prompt が意図的に commit を作成する、または Git `HEAD` を動かす必要がある場合に限り `allow_commit: true` を設定してください。省略時および `false` では guard が有効です。guard は従来の `command:` mode とすべての SDK backend（`jcode`、`claude`、`opencode`）を対象とします。command/option step には影響しません。ref 拒否の対象は step が動作する repository で、git common dir により識別します。その repository の main checkout と関連するすべての worktree における `HEAD` および `refs/heads/*` の更新が拒否されます。repository 限定の commit guard は、step 内で作られる使い捨て repository（一時 repository、test suite の repository、別 clone、nested repository）の ref 更新を許可しますが、SDK shell prefix 拒否はこれらの repository を対象とする一致 Git command も拒否します。guard 対象 repository の checkout または linked worktree 内の commit は引き続き拒否されます。検出された ref 移動は step を失敗させます。cruise が index や worktree を reset せず、元の branch reference を復元する場合があります。

`allow_commit` を省略するか `false` にした場合、編集を未 commit のまま残し、作業中 repository と linked worktree の `HEAD` または local branch ref を動かさず、task から要求されても commit guard を回避しないよう agent に指示を追加します。この notice は Git enforcement を補います。`allow_commit: true` では commit-guard notice を省きますが、明示的な `disallowed_tools` による制限 notice は引き続き追加されます。

`allow_commit: true` は `group:` や `workflow_call:` 呼び出しではなく、実際の prompt に設定してください。guard は step の child 環境に `core.hooksPath` と `CRUISE_GUARD_COMMON_DIR` を注入して実装されます。これにより `reference-transaction` hook を設定し、指定変数の repository 内でのみ ref 更新を拒否します。拒否処理だけが repository に限定されます。注入した hooks path は step の下で実行されるあらゆる repository の Git invocation に適用されるため、guard 対象外で ref 更新できる無関係な repository でも、その repository 独自の hook (`pre-commit`、`commit-msg`、`pre-push` など) は実行されません。hook は Git が hook に渡す `GIT_DIR` から現在の repository を特定し、linked worktree では `commondir` file をたどります。特定できないときは `git rev-parse --git-common-dir` に fallback します。guard は fail-closed です。transaction の repository を解決できない場合は transaction を拒否し、commit guard violation で step を失敗させます。一方、working directory がないか Git worktree 外の directory であれば何もせず正常に進みます。同一ユーザーに対する best-effort policy であり security boundary ではありません。他 repository の状態を事後検査しません。

長い prompt は `prompt_file` で file から読み込めます。

```yaml
steps:
  implement:
    prompt_file: prompts/implement.md
```

`prompt_file` には絶対 path、`~/` path、設定 file からの相対 path、GitHub blob/raw URL を指定できます。bare file 名は設定 file と同じ directory から解決されます。GitHub URL は設定 load 時に `gh api` で取得されます。`workflow_call` 内で参照する file は、呼び出された workflow の directory を基準に解決されます。file 内容はそのまま保持され、`prompt` と同じ変数展開を行います。`prompt` と `prompt_file` は相互排他です。repo-backed session で設定が一時 clone にある場合、session snapshot は解決済み prompt 内容を保存するため clone 削除後も使用できます。

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
- 各 child は block 開始前の同じ input と `{prev.*}` 値を受け取ります。環境の優先順位は workflow < parallel block < child です。child には `model`、`env`、`skip`、`when`、`timeout` を指定できます。prompt child のみ `instruction`、`computer_use`、`permission`、`disallowed_tools`（parent には設定不可）、および block 内で別名にした artifact を保存する `output_file` も指定できます。parallel の `instruction` は非対話であり、実行時 input が空でない必要があります。input を指定するか、その前に順次実行する instruction step を置いてください。そうしないと入力 editor を開かずに実行エラーになります。command child に `instruction` は指定できません。`prompt_file` は通常どおり設定 file から解決されます。command array は child ごとに順次実行されます。
  同一 block の sibling が生成した artifact (`{file:...}` が他 child の `output_file` を参照する場合) を読む child prompt は、設定検証時に拒否されます。空の `parallel: {}` block と、空の `command:` array を持つ child も拒否されます。
- join 後の `{prev.output}` は YAML 宣言順で child 名を key にした JSON object です。各 entry は `output`（prompt text。command は `null`）、`stderr`（実行 error を含む）、`success`、`skipped` を持ちます。すべての child が成功または skip の場合 `{prev.success}` は true です。`{prev.stderr}` は child 名とともに error を連結します。log には `[parent/child]` prefix が付きます。command child に対話 stdin はありません。
- child の失敗で sibling は取り消されません。通常 step の failure semantics に従い、block 全体が1 step、最大1 failure として扱われます。parent に `if.fail` または `next` を設定してください。retry では block 全体を再実行します。未処理の prompt/execution error は sibling 完了後に workflow を停止します。nonzero command exit と timeout は通常の step failure 経路を使います。
  ただし2種類の child error は上記集約より優先されます。commit-guard violation または jcode prompt の missing-model error は常に致命的であり、parent に `if.fail` があっても workflow を停止します。一方、child timeout 単独では致命的にならず、常に `{prev.*}` と通常の step failure 経路から報告されます。
- child timeout の影響はその child のみに及びます。parent timeout は未完了 child を cancel し、failure 経路へ進む前に停止を待ちます。Ctrl+C はすべての child を停止します。resume では既に完了した child を含め、中断した block 全体を再開します。graph 表示と step 選択では block を1 step として扱います。
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

- **Command step:** child process を kill し、step を失敗（non-zero exit）として扱います。`{prev.success}` は `false` となり、workflow は通常の failure 経路（下記 `if.fail`）に進みます。
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

ここにある prompt step は `github-review` prompt を含め workflow `disallowed_tools` を継承し、step で制限を追加できます。native `squash-merge` は prompt ではなく、空でない `disallowed_tools` list を持てません。

GitLab では、組み込み `after-pr` step は `{pr.url}` 内の `/-/merge_requests/` を検出し、`gh` の代わりに `glab`（`glab mr update --ready`、`glab ci get --merge-request`、`glab ci trace`）を使います。

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

`after-pr` の prompt step にも同じ default と repository scope の commit guard が適用され、fail-closed 動作をします。組み込み `resolve-conflict` と `fix-ci-error` prompt は PR branch 上で修正を commit する意図があるため明示的に `allow_commit: true` を設定しています。session repository 内で `HEAD` を進める必要がある他の after-PR prompt は個別に opt in する必要があります。command/option step には影響しません。

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
      Fix the review comments and commit the changes.
      Reply with the requested JSON describing each thread.
    allow_commit: true
    timeout: 30m                    # required; bounds the whole loop
    output_file: review-report.json # optional iteration report in the session artifacts
```

bot mode では各 iteration で、列挙した bot が現在の head commit を review するまで待ってから、その bot が開始した未解決 thread をすべての page から取得します。all-reviewer mode では新しい review を待たず、対象となる未解決 thread を直ちに取得します。どちらの mode も head OID と thread data を prompt に追加して実行します。prompt は JSON のみで回答しなければなりません: `{"actions":[{"thread_id":"...","reply":"optional","resolve":true}]}`。列挙された thread ID のみ受け付け、各 ID は最大1回です。`viewerCanReply` / `viewerCanResolve` が許可する場合に限り、空でない `reply` または `resolve: true` が必要です。reply の投稿と thread の解決は Cruise 自身が `gh api graphql` で行います（認証済み `gh` が必要）。prompt が作った commit は push され、未 commit 変更が残ると step は失敗します。対象となる未解決 thread がなくなると loop を終了します。`max-iterations` 到達時に thread が残る、timeout、API error、response error は失敗です。他の `after-pr` error は warning のみですが、`github-review` failure は run を `Failed` にします。jcode after-pr prompt の cancellation と missing-model error も伝播し、missing-model error は run/session を失敗させます。reply には idempotency marker があるため、安全に再実行できます。この step は `after-pr` の直下でのみ有効です。

既定の `reviewers: bots` はこの動作を維持し、空でない一意な `bots` list が必要です。既存の human と bot の feedback を処理するには `github-review: { reviewers: all, max-iterations: 3 }` を使い、`bots` を省略するか空 list にします。この mode は新しい review を待ちません。thread text を agent に渡す前に Cruise は各 author の repository collaborator permission を確認し、`write`、`maintain`、`admin` の author のみを対象にします。404 は permission なしを意味し、それ以外の permission lookup failure では step が失敗します。除外した未解決 thread ID は report に記録されますが、成功を妨げません。対象となる未解決 thread がなくなると loop は成功し、iteration 後も対象 ID 集合が変わらない場合、または iteration limit 到達時に対象 thread が残る場合は失敗します。組み込み PR review workflow は、この mode を60分 timeout、3 iteration、`allow_commit: true` で使用します。

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

> **注意:** `{model}` は template 変数ではありません。top-level `command` array 内でのみ解決される特別な placeholder です。`prompt`、`prompt_file`、`instruction`、command step field 内では利用できません。

literal の波括弧は Rust `format!` 形式で escape します。`{{` → `{`、`}}` → `}` です（例: `"{{input}}"` は `input` の lookup ではなく literal string `"{input}"` に解決されます）。閉じていない `{`、単独の `}`、空の `{}`、未定義変数の参照は template 構文エラーです。
