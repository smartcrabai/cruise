[English](../en/workflow-control.md) · [日本語](../ja/workflow-control.md) · [简体中文](../zh/workflow-control.md) · [目次](README.md)

# Workflow flow control と合成

<a id="flow-control"></a>
## Flow control

<a id="explicit-next-step"></a>
### 次の step を明示する

```yaml
steps:
  step_a:
    command: echo "hello"
    next: step_c                  # jump over step_b
  step_b:
    command: echo "skipped"
  step_c:
    command: echo "world"
```

`next` を省略するか null (`~`) にすると、宣言順で次の step に進み、最後の step の後で workflow が終了します。`next` に値を指定すると、名前付き step にジャンプします。

<a id="skipping-a-step"></a>
### Step を skip する

```yaml
steps:
  optional_step:
    command: cargo fmt
    skip: true                    # always skip

  fix_errors:
    command: cargo fix
    skip: prev.success            # skip if the variable "prev.success" resolves to "true"
```

<a id="workflow-flags"></a>
### Workflow flag

top-level `flags:` で flag を宣言し、step の `flag: <name>` で結び付けます。flag が `on` の場合に限り対象 step が実行され、`off` では skip されます。

- **設定可能な対象:** 通常 step、group call site、`workflow_call` call site（展開された全 step に適用）、`parallel` parent、`after-pr` 内の step。group 定義内の step、呼び出された workflow 独自の step、parallel child には `flag:` を設定できません。
- **既定は off:** 宣言済み flag は値がない場合すべて off です。
- **値の指定:** `--flag NAME` は1つの flag を有効化します。`--flag NAME1,NAME2` は comma 区切りの複数選択を有効化し、`--flag` option は繰り返し指定できます。`NAME=on|off|true|false` を明示すると個別値を設定／解除できます。`cruise run` は保存済み値と CLI override を使い、prompt は表示しません。flag が宣言されていて `-f/--flag` がない TTY では `cruise exec` と `force_exec` entry point が prompt を表示します。ただし dry-run は除きます（[Workflow flag](cli-reference.md#cruise-run)参照）。未知の名前は拒否されます。
- **Resume:** 保存済み session 値は維持され、指定された `--flag` は名前が一致する flag のみ上書きします。
- **skip との統合:** flag で無効になった step はユーザーが選択した `skipped_steps` と統合され、いずれかに該当すれば skip されます。

`skip` field は static boolean (`true`/`false`) または変数参照文字列を受け付けます。変数参照の場合、その時点の値が `"true"` に解決されれば step を skip します。

<a id="conditional-execution-by-file-existence-whenexists"></a>
### file の有無による条件実行 (`when.exists`)

`when.exists` は実行前条件で、指定 glob に一致する file が少なくとも1つある場合を除き step を **skip** します。関連 file がある場合のみ step を実行するために使います。たとえば `.rs` file がない repository では何もしない Rust 専用 review step を作れます。

```yaml
steps:
  rust-review:
    when:
      exists: "**/*.rs"       # run only if a matching file exists; otherwise skip the step
    prompt: "Review the Rust code and fix any issues."
```

- glob は workflow の working directory から解決されます。絶対 pattern はそのまま使用します。
- pattern 内の template 変数は glob 判定前に解決されるため、`exists: "{input}/**/*.rs"` のように指定できます。
- 空 glob は設定検証時に拒否されます。
- `when.exists` と `skip` は独立しています。`skip` が既に step を skip している場合、glob は評価されません。

<a id="conditional-execution-file-changed-detection"></a>
### file 変更検出による条件実行

step に `if: file-changed: <target>` がある場合、実行後に file の変更があれば workflow は `<target>` へジャンプします。変更がなければ通常どおり次の step に進みます。

review step が code を変更した場合に test を再実行するなど、ループバックに使います。

```yaml
steps:
  test:
    command: cargo test

  review:
    prompt: "Review the code and fix any issues."
    if:
      file-changed: test    # after review, if it modified files, jump back to test
```

<a id="no-file-changes-detection-ifno-file-changes"></a>
### file 変更なしの検出 (`if.no-file-changes`)

step の `if.no-file-changes` を `retry` または `failed` に設定すると、workspace file を変更せず step が完了した場合、指定された処理を行います。

- **`failed`** — workflow を error で中止し、session を `Failed` state に遷移させます。LLM が実装したと報告しても実際には file を変更していない場合の検出に役立ちます。
- **`retry`** — 現在の step を再実行します。意味のある file 変更が起こるまで再試行する場合に便利です。

```yaml
steps:
  implement:
    prompt: "Implement the feature described in {plan}"
    if:
      no-file-changes: failed

  fix:
    prompt: "Fix the issue"
    if:
      no-file-changes: retry
```

**制約:**
- 値は `retry` または `failed` のどちらかです。
- `after-pr` step では使用できません（validation 時に拒否）。
- group level（group 定義内の `if`）では使えません。
- 同じ step の `if: file-changed` と併記できますが、`no-file-changes` が無効化します。その step について `file-changed` edge は compile されず、target への jump も起こりません。

<a id="declaring-intentional-no-changes"></a>
#### 意図した変更なしを宣言する

変更がないことを常に回避すべき失敗とみなす必要はありません。plan が明示的に変更しないよう指定する場合（例: 「ここでは test を追加しない」）、agent が retry のたびに同じ結論を出すのは停滞ではなく正しい応答です。変更なしが意図的だと cruise に伝える方法は2つあり、どちらもその試行の `failed` と `retry` の**両方**を無効にします。

- **Output marker** — step の raw output 内で `NO_CHANGES_INTENTIONAL: <reason>` から始まる行（行頭の空白は無視）を出力します。単なる text matching なので tool support 不要で、`command:` と3つの `sdk:` mode すべてで動作します。marker は行頭に必要です。行の途中（引用、コード block 内など）に現れても対象になりません。
- **構造化 SDK response** — SDK mode の agent は、`if.no-file-changes` 条件を持つ prompt step で schema 検証済み structured response を使い、意図的な変更なしを宣言できます。従来の `command:` mode では output marker を使ってください。

<a id="failure-handling-iffail"></a>
### Failure の処理 (`if.fail`)

`if.fail` は回復可能な step failure が起きたときの処理を決めます。対象には command step の non-zero exit code、回復可能な prompt error（LLM transport error を含む）、`timeout`、`no-file-changes: failed` の発動があります。

- **`fail: <step-name>`** — 指定した step にジャンプします。
- **`fail: { retry: true }`** — 現在の step を再実行します。

```yaml
steps:
  flaky_test:
    command: cargo test --flaky
    timeout: 2m
    if:
      fail:
        retry: true        # retry on non-zero exit, timeout, or other failure

  deploy:
    command: ./deploy.sh
    if:
      fail: rollback       # jump to the `rollback` step on failure

  rollback:
    command: ./rollback.sh
```

`if.fail` は他の flow-control jump と同じ loop-protection budget（`--max-retries`、なければ top-level `max_retries`、それもなければ3）に従います。budget は user-skip fallback と group-retry-exhausted の exit を除く graph edge すべてを対象にします。そのため通常の順次遷移や明示的 `next`/option jump も数えられます。いずれか1 edge の通過回数が上限を超えると、設定ミスの loop が永続しないよう loop protection により run を停止します。

**制約:**
- group level と `squash-merge: true` の native step を除くすべての `after-pr` step では `if.fail` を拒否します。native `squash-merge: true` が受け付けるのは名前付き jump のみです。
- 通常 step では `if.fail` を `file-changed`、`no-file-changes` など他の `if:` key と組み合わせられます。native squash-merge では名前付き `if.fail` 経路のみ受け付けます。

<a id="quality-gates-quality_gates"></a>
### Quality gate (`quality_gates`)

prompt step の成功した試行後、遷移を適用する前に作業を検査する command を空でない list として指定します。

```yaml
max_retries: 2
steps:
  implement:
    prompt: Implement the requested change.
    quality_gates:
      - command: cargo test
      - command: cargo clippy
        timeout: 10m
    if:
      fail: recover
    next: done
  recover:
    prompt: Investigate the failed implementation.
  done:
    command: echo done
```

各 entry には空白でない `command` が必要です。通常の秒、`m`、`h` 構文による正の `timeout` を指定できます。command は prompt の成功試行後に順次実行され、最終 step でも実行されます。最初の failure で停止します。skip された step、prompt が失敗した試行、`if.no-file-changes: retry` により破棄された試行では gate は実行されません。保留中の `if.file-changed` 遷移は gate 通過時のみ適用されます。

gate 失敗時は通常遷移を適用する前に同じ prompt step を retry します。次の prompt は command output の無害化済み・サイズ制限付き抜粋を untrusted data として受け取ります。gate output は `{prev.*}` を置き換えません。

retry は既存の edge ごとの budget（`--max-retries`、なければ top-level `max_retries`、それもなければ3）に従い、同じ step の他の self-retry と共有します。上限到達時に `if.fail: <step>` があればその step にジャンプし、それ以外は cruise が `QualityGateExhausted` で停止します。

prompt と `prompt_file` step（group 内の個々の step を含む）には gate を指定できます。parallel parent/child、`after-pr` step、group または `workflow_call` call site、prompt 以外の step では拒否されます。gate command は通常の `command` step と同様に shell command を実行します。信頼できる workflow 設定のみで使用してください。

<a id="step-groups"></a>
## Step group

複数 step 間で retry loop を調整するために step を group 化できます。`if: file-changed` 条件が発動した場合、group は全 member step をまとめて再試行します。

group の step は inline 定義し、main `steps` section から `group: <name>` で呼び出します。

```yaml
max_retries: 4

groups:
  review:
    if:
      file-changed: test    # if any step in the group changes files, jump to `test`; sequencing then re-enters the group
    max_retries: 3          # maximum number of group-level retry loops (optional)
    steps:                  # steps defined inside the group
      simplify:
        prompt: /simplify
      coderabbit:
        prompt: /cr

steps:
  test:
    command: cargo test

  review-pass:
    group: review           # invokes the "review" group's steps at this point
```

同じ group を workflow 内の複数箇所から呼び出せます。

```yaml
steps:
  test-lib:
    command: cargo test --lib
  review-lib:
    group: review

  test-doc:
    command: cargo test --doc
  review-doc:
    group: review           # same group, different call site
```

**制約:**
- group 定義内の step に nested `group:` reference や個別 `if:` 条件を設定できません。group-level `if:` が group 全体に適用されます。
- group の `if: file-changed` 条件が発動すると設定 target step（例では `test`）へ移動し、通常の順次実行で group に戻って各 step が再実行されます。
- call-site step（例: `review-pass: group: review`）には独自の `if:` 条件を設定できません。
- group call site には `allow_commit: true` を指定できません。内側の prompt step に設定してください。
- group call site には `output_file` も指定できません。内側の prompt step を使うよう専用 error が案内します。
- group call site には `computer_use` を指定できません。内側の prompt step に設定してください。
- group 定義内の step に `workflow_call:` を指定できません。呼び出す workflow は top-level で展開してください。
- group 定義内の step に `flag:` を指定できません。group call site に flag を指定してください。

<a id="workflow-composition-workflow_call"></a>
## Workflow composition (`workflow_call`)

step では `prompt`、`prompt_file`、`command`、`option`、`parallel` の代わりに `workflow_call` を設定し、別の workflow config file に処理を委譲できます。呼び出した workflow の step は call site に inline 展開され、各 step ID には call-site 名の prefix が付きます（例: `shared-review/simplify`）。

```yaml
steps:
  build:
    command: cargo build

  shared-review:
    workflow_call: ./workflows/review.yaml    # local relative path

  deploy:
    command: cargo publish
```

参照先 file は通常の cruise config です。その top-level execution 設定（`command`、`sdk`、`model`、`env`、`computer_use` など）は無視され、`steps` のみ import されます。展開された step には parent の設定が適用され、呼び出された workflow 内の per-step override は保持されます。

<a id="required-string-inputs"></a>
### 必須 string input

callee の完全な例 `workflows/review.yaml`:

```yaml
inputs: [review_prompt, label]
steps:
  review:
    prompt: "{inputs.review_prompt}"
  announce:
    env:
      REVIEW_LABEL: "{inputs.label}"
    command: 'printf "%s\n" "$REVIEW_LABEL"'
```

caller の完全な例 `cruise.yaml`:

```yaml
create_pr: false
steps:
  shared-review:
    workflow_call: ./workflows/review.yaml
    with:
      review_prompt: "Review the changes for {input}, using {plan}."
      label: "review"
```

`inputs` は省略可能な名前 list で、`[A-Za-z_][A-Za-z0-9_]*` に一致する一意な名前が必要です。宣言されたすべての input は必須です。`with` は string value の mapping です。空文字列は有効ですが、number、boolean、array、object は string に変換されません。不足／余分な argument と未宣言 input の参照は実行前に失敗します。`with` を省略するか `null` にすると argument を渡しません。空 mapping も `workflow_call` step 以外では使用できません。直接 load された root は空でない `inputs` を宣言できず、user/project `default.yaml` でも `inputs` は禁止です。

`{inputs.name}` は config load 時に prompt text（load 済み `prompt_file` 内容を含む）、instruction、command string/array element、env value、nested call の `with` value に bind されます。path、step 名、遷移、flag、model の parameter 化には使えません。

`{input}`、`{plan}`、`{prev.*}`、`{pr.*}`、`{file:...}` など runtime token は通常の実行時評価まで残ります。input は template fragment であり、secret store や自動 shell quoting 機能ではありません。

<a id="return-labels"></a>
### return label

呼び出された workflow は名前付き exit を宣言でき、caller が遷移先を決めます。例: `workflows/review-gate.yaml`:

```yaml
returns: [approved, needs_fix]
steps:
  review:
    command: "cargo test"
  decide:
    option:
      - selector: Approve
        return: approved
      - selector: Request changes
        return: needs_fix
```

caller は宣言された各 label を、自身の step map 内の step に対応付けます。

```yaml
steps:
  gate:
    workflow_call: ./workflows/review-gate.yaml
    on_return: {approved: publish, needs_fix: fix}
  fix:
    prompt: "Fix the review findings in {plan}"
  publish:
    command: "git push"
```

`returns` は `[A-Za-z_][A-Za-z0-9_-]*` に一致する一意 label の list です。宣言された各 label は callee return で使われ、caller で対応付けられる必要があり、余分な mapping は認められません。`on_return` は `workflow_call` のみで使え、`null` は省略と同じです。遷移先は同じ parent step map に存在する必要があり、workflow call site は指定できません。callee の top-level step には `next` の代わりに `return` を指定できます。option step では各 option に `return` を指定します。return は同じ step/option の `next`、または return を持つ step の `if.file-changed` と併用できません。group/parallel child、call site、after-PR step に return は指定できません。

return なしで通常完了すると、call-site `next` があればそこへ進み、なければ次の parent step に続きます。return-bearing step が skip されても return にはなりません。call site に `next` が指定されていても、宣言順の次へ進みます。callee の最終 step に step-level return がある場合、call-site `next` は無効です。return が option 上だけにある場合は指定できます。

<a id="supported-sources"></a>
### 対応 source

| Source | 例 |
|--------|---------|
| ローカル相対 path | `workflow_call: ./workflows/review.yaml` |
| GitHub blob URL | `workflow_call: https://github.com/org/repo/blob/main/workflows/review.yaml` |
| GitHub raw URL | `workflow_call: https://raw.githubusercontent.com/org/repo/main/workflows/review.yaml` |

`cruise workflow eject <name> [--to user|project]` は call を展開し、file-backed prompt を内容に置き換えた、自己完結して編集可能な workflow を書き出します。

<a id="call-site-fields"></a>
### Call-site field

`workflow_call` step は純粋な委譲地点です。`workflow_call` と併せて設定できるのは `with`、`on_return`、`skip`、`when`、`next`、`flag` のみです。

- `skip` と `when` は展開後の**最初**の step に適用されます。
- `next` は展開後の**最後**の step に、その step が独自の `next` を持たない場合に適用されます。
- `flag` は展開されたすべての step に適用されます。呼び出された workflow 独自の step に `flag:` は指定できません。

その他すべての step field (`prompt`、`prompt_file`、`command`、`model`、`instruction`、`plan`、`option`、`if`、`timeout`、`env`、`output_file`、`group`、`parallel`、`computer_use`、`permission`、`disallowed_tools`) と `allow_commit: true` は validation 時に拒否されます。error には問題のある全 field が列挙されます。明示的な `allow_commit: false` は省略と同じです。

<a id="nesting-and-cycle-detection"></a>
### Nesting と cycle 検出

workflow call は nested にできます。呼び出された workflow がさらに `workflow_call` step を持てます。step ID には prefix が累積します（`outer/inner/step`）。循環参照（A が B を呼び、B が A を呼ぶ）は検出され拒否されます。group と `workflow_call` はどちらの方向でも併用できません。呼び出された workflow 内の group は非対応で、group 定義内の step から `workflow_call` もできません。

```yaml
# parent.yaml -> nested/outer.yaml -> inner/leaf.yaml
# Results in step IDs: outer-call/leaf-call/leaf
steps:
  outer-call:
    workflow_call: ./nested/outer.yaml
```
