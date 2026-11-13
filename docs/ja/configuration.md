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

   `add-tests` は本番動作と実際のユーザージャーニーを棚卸しし、安定した網羅・不足 ID をソース境界とテストアサーションに対応付けてから、有用なテストと該当する CI を追加または完成させます。`add-formal-specs` は実行可能な Quint/Alloy モデルと、実装に接続した step definition を持つ Gherkin シナリオを追加し、CI で実行して、要件を弱めることなく反例や実際のソース・仕様の欠陥を修正します。

   step の順序だけでは、先行レポートがプロンプトに挿入されることはありません。`add-formal-specs` は監査およびモデルの出力を session artifact (`formal-specs/audit.md`、`formal-specs/model.md`) として保存し、後続プロンプトは必要なレポート内容を `{file:...}` で明示的に埋め込みます。

   `add-tests` は実装に元の依頼 (`{input}`) と保存済み plan を `{file:test-ci-gap-plan.md}` 経由で渡します。検証では `{input}`、`{file:test-ci-gap-plan.md}`、`{file:test-ci-implementation.md}` を独立して読み、計画・対応した各 ID を実際のテスト、アサーション、コマンドに照らして再確認します。`test-ci-gap-plan.md`、`test-ci-implementation.md`（各 ID の applied/already-covered/rejected/blocked 判定と根拠）、`test-ci-verification.md` を出力します。網羅台帳は動作を `Covered`、`Partial`、`Missing` と記録し、除外範囲と未レビュー範囲を理由付きで記録して、数値の網羅率ノルマを課さずに対応可能な不足を優先します。

   組み込み `tdd`、`refactor`、`add-tests` はリポジトリの指示に従い、対象範囲に含まれる実製品ターゲット（Windows、Linux、ネイティブターゲットを含む）ごとに、適切な既存ツールをまず再利用します。そのバージョン、コマンド、セットアップ、CI を維持します。以下の推奨は、適切な機能が存在しない場合に限るもので、許可リスト、移行、必須インストール、実行時依存、新しい workflow/config key ではありません。Maestro など既存ツールも引き続き有効です。スタックでサポートされる互換バージョンを使い、新しいプラットフォームテストマトリクスを強制しないでください。

   バックエンドおよびコマンドラインの機能に関する推奨:

   - **破棄可能な外部依存関係:** [Testcontainers](https://testcontainers.com/)（および[Rust ライブラリ](https://rust.testcontainers.org/)）は、必要な場合に、スタック対応ライブラリを使って実際の外部データベースや依存サービスを使い捨てでプロビジョニングするものです。互換性のあるライブラリとイメージのバージョンを固定し、起動を待ち、動的に割り当てられた接続情報、実際のアプリケーションマイグレーション、分離したシードデータ、テスト所有リソースに限定した後片付けを使います。これはデータベースアサーションフレームワークではなく、インプロセス／組み込み DB や DB を使わないプロジェクトでは不要です。必要な Docker 互換ランタイムがなければ、ホストの本番 DB、モック、見せかけの成功ではなくブロックとして報告してください。
   - **CLI と TUI:** [Bats-core](https://github.com/bats-core/bats-core) は、実際にビルドした CLI 実行ファイルを起動し、終了ステータス、stdout、stderr、副作用を意味のある形でアサートする Bash テストを保存できます。Bats の `run` はコマンドを捕捉した後に成功を返すため、対象コマンド自身のステータスを明示的にアサートしてください。`run` だけでは合格になりません。実際の対話 TUI には、Bats で実際の PTY 対応操作ハーネス（適切なら Expect など既存のものを再利用）を調整し、キーを送信してターミナル画面／状態遷移をアサートし、該当する場合は描画済みターミナル／画像の証拠を確認します。pipe、非対話 stdout、`--help` のスモークテストは TUI E2E ではありません。必要な PTY／ランタイムがなければブロックとして報告してください。サポート対象の Bash とホストプラットフォームを守り、未対応のネイティブ Windows Bash を強制しないでください。
   - **Web HTTP API:** [Hurl](https://hurl.dev/) は保存済み `.hurl` テストを `hurl --test` で実行し、実際のローカルアプリケーション HTTP endpoint に対して実質的な応答と関連する状態・効果をアサートします。既存のセットアップ、分離、ローカルテスト認証を再利用し、適切な場合は Testcontainers で DB 依存を用意します。静的な schema/parser 検査やモック HTTP は実 endpoint テストではありません。実資格情報や外部の本番サービスを使わないでください。
   - **バックエンドサービス契約:** [Pact](https://docs.pact.io/)（[Pact が役立つ場合](https://docs.pact.io/getting_started/what_is_pact_good_for)）は、特定され、制御可能で、サポートされる実際の consumer/provider がある場合に限り、HTTP またはメッセージ契約に使います。実際の consumer client code で契約を生成し、明示的な provider state を用いて実際の provider 実装で検証し、両側を記録します。consumer mock や pact-file の生成だけでは検証完了になりません。Pact は一般的なバックエンド機能、DB、統合 E2E ではなく、Hurl や実経路検査の代替ではありません。制御されたプロトコル契約用の double は有効な契約であり、mock のみの E2E ではありません。Broker/PactFlow、クラウドインフラ、secret を必須にしないでください。

   UI ツールを選ぶ前に、manifest とソースから実際の製品とフレームワークを分類します。ブラウザー向けには、該当機能が不足している場合に限り、視覚検査に Playwright CLI (`playwright-cli`)、保存 E2E に Playwright Test (`*.spec.ts`、`npx playwright test`) を推奨します。SwiftUI の iOS/macOS では、適切な視覚ツールが不足している場合に限り、実アプリ操作とスクリーンショット取得に Xcode MCP（Preview だけでは不十分）、適切な保存 UI E2E ツールが不足している場合に限り XCTest/XCUITest (`xcodebuild test`) を推奨し、実際の intent 統合に限って App Intents Testing を追加します。Android では、該当する既存機能が不足している場合に限り視覚検査に mobile-mcp、Views に Espresso、Compose に Compose UI Test、システム／アプリ間フローに UI Automator を推奨します。

   React Native/Expo の iOS+Android では、該当機能に適切な既存ツールがない場合に限り、視覚検査に agent-device CLI または接続済み MCP、保存 E2E に Detox + Jest (`detox build`/`detox test`) を推奨します。Expo では Detox 統合済みネイティブビルド、Expo/RN/New Architecture/Detox の互換性固定、および prebuild/config-plugin ワークフローが必要です（Expo Go や生成ファイルを手で編集するだけの方法では不可）。適切に iOS Simulator とサポート対象 Android ランタイムを使い、実機 iOS 一律の網羅を要求しないでください。Detox 互換性がなければ、別 runner への切替ではなくブロックです。Electron では、該当機能に適切な既存ツールがない場合に限り、実アプリ操作に Playwright Electron API、保存 E2E にその API を使う Playwright Test を推奨します。experimental API／バージョン互換性を確認してください（renderer-in-browser はネイティブアプリ合格ではなく、OS dialog stub は実 OS dialog の検証ではありません）。

   Tauri は通常のブラウザーや Electron frontend ではなく、WebView + Rust/IPC のネイティブアプリとして分類します。互換ホストで適切な視覚ツールが不足している場合は、実アプリへの入力とスクリーンショット取得に [tauri-plugin-mcp](https://github.com/P3GLEG/tauri-plugin-mcp) を推奨し、続けて画像を実際に確認します。適切な永続 E2E ツールが不足している場合は、必須の companion [tauri-plugin-webdriver](https://github.com/Choochmeque/tauri-plugin-webdriver) と、実際の WebDriver client/test runner（既存のものを再利用し、なければスタック互換の WebdriverIO または Selenium など）を伴う [tauri-webdriver](https://github.com/Choochmeque/tauri-webdriver) を推奨します。互換するバージョンまたは Git revision を固定し、ホスト、Tauri、WebView、client の互換性を確認します。実 frontend から Rust/ネイティブ挙動への関連する経路を実行する、実質的な assertion と独立したコマンドを保存してください。両 plugin は開発／テスト専用です。自動操作 endpoint を本番 build から除外し、ローカル／分離済みテストセッションを使い、MCP をグローバルに有効化しないでください。mock、synthetic/untrusted event、dialog stub を考慮してください。MCP plugin は既定で JavaScript `alert`/`confirm`/`prompt` を stub 化するため、実際の native dialog の動作証明にはなりません。必要なホスト／ランタイム／plugin 機能がなければブラウザーだけの合格で代用せず、ブロックとして報告してください。

   該当するフレームワーク推奨がないネイティブターゲット／機能では、適切な既存ツールのみを再利用し、新しいツールを導入しないでください。確立済みの必須ツールがあっても互換ホスト、ランタイム、サーバーが利用できなければ、正確な阻害要因を報告します。視覚操作／スクリーンショットまたは保存 E2E 機能のどちらか一方しかなければ、実行可能な作業を行い、不足機能を正直に報告します。両方を実施したと主張してはいけません。対象 UI フローごとに、意味のある状態・失敗時の実アプリ操作、保存して画素を検査したスクリーンショット、およびリポジトリに保存される決定論的で assertion を含む E2E ソースが必要です。テスト状態は reset/分離され、正確で再現可能な build/run コマンドと、事前探索セッションや LLM の判断に依存しない実行を備えます。正確なコマンド／結果と検査したスクリーンショットを共通証跡に再利用し、証跡の重複作成のためだけに再実行しないでください。実際の E2E 1 回と検査済みスクリーンショットで十分であり、重複実行や pixel-diff/golden test は不要です。視覚証拠だけでは E2E の網羅を完了しません。関連する製品 UI がない場合は、その制約を正直に報告してください。サポート対象ホストのブラウザー／Electron 検査と CI は対象であり、Linux web CI を無効化してはいけません。

   実行エージェントが提供する仕組みで関連する利用可能な skill を調べて読み込み、適切な場合は接続 MCP server を使用してください。skill は任意で、未提供でもそれだけで阻害要因ではありません。skill を追加したり API を推測したり、グローバル MCP/config 設定を編集したり、computer-use 設定を強制したりしないでください。必須機能に適切な既存ツールがない場合は上記の該当推奨を使います。必要な機能／server／runtime が利用できなければ成功を主張せず、具体的な阻害要因を報告します。

   `research` は plan → investigate → independent review → synthesize の4つの限定された読み取り専用フェーズで動作します。`research/plan.md`、`research/evidence.md`、`research/review.md`、`research/report.md` を保持し、各フェーズに `{input}` を渡し、後続フェーズは `{file:...}` で先行レポートを明示的に読みます。最終レポートは根拠付きの事実、分離した推論、矛盾、不確実性、レビュー判定、正確な阻害要因を答えます。外部の主張には URL、正確な位置、入手可能なソース版／日付、実際の取得日を記載します。リポジトリの主張にはパスと行範囲を記載します。ソースや実施していない検査を捏造しません。組み込みワークフローは `create_pr: false` とし、永続セッションで作業ツリー／コードを変更せず、自動 commit/push/PR/`after-pr`/PR cleanup を省略しながら session artifact と worktree は保持します。`merge` はこの読み取り専用・no-PR 方針を覆しません。明示的に選ばれた利用不能な操作は理由付きで省略します。`cruise plan --skip-planning -c builtin:research "question"` を使い、求められた場合は plan を承認してから `cruise run <session-id>` を実行してください。transient `exec` や `force_exec` は使いません。

   `security` は範囲と網羅を棚卸しし、code/dependency/delivery の並列監査、トリアージ、確認済みで安全かつ対象内の欠陥の修正、各 finding の独立検証を行います。`security/scope.md`、`security/code-findings.md`、`security/dependency-findings.md`、`security/delivery-findings.md`、`security/worklist.md`、`security/repairs.md`、`security/verification.md` を保持します。各フェーズに `{input}` を渡し、後続フェーズは `{file:...}` で各レポートを明示的に消費します。triage には結合した監査結果を `{prev.output}` として渡します。finding は安定 ID と、悪用可能性・根拠、accepted/rejected/blocked の判定、false positive、修正、回帰検査、独立した終了確認を保持します。レポートは未実施検査、除外／未レビュー範囲、制限を正直に示し、システム全体のセキュリティを検証したと主張しません。

   `research` workflow と `security` の inventory/audit/triage フェーズには読み取り専用の SDK permission が必要です。workflow と適用される user/project default の解決後に `command:` も `sdk:` も残らない場合、Cruise は既定の `sdk: jcode` を使います。`command:` backend は読み取り専用に制限できず、制限付き permission step では拒否されます。

   対話 selector（CLI、TUI、WebUI）は組み込みやその他の候補をカテゴリ別に表示します。設定ファイルがない非対話解決では `tdd` が使われます。

   `create_pr` は workflow 専用の boolean で、既定値は `true` です。`false` にすると通常の session、artifact、worktree は保持したまま、Cruise による自動 commit、push、PR 作成、`after-pr`、PR ベースの cleanup を省略します。ローカルの no-PR worktree には `gh`/`glab` は不要ですが、`--repo` ではリポジトリアクセスに forge CLI が引き続き必要です。PR 作成が有効で、成功した run に新しいブランチ commit がなければエラーなしで PR 作成を省略しますが、既存のブランチ commit は引き続き対象です。明示的に有効化した after-PR 処理は PR が作られない場合、理由付きで省略されます。`merge` は `create_pr: false` や `research` など読み取り専用 workflow を覆しません。明示的な `if.no-file-changes` の failure/retry guard はそのまま適用されます。`create_pr` は user/project `default.yaml` では拒否され、CLI option ではありません。

2. **`CRUISE_CONFIG` 環境変数** — 設定されていればその値を直接使います（ファイルが存在しない場合はエラー）。選択プロンプトは表示されません。
3. それ以外では cruise は以下の場所からすべての候補を収集し、選択肢として提示します。
   - 現在のディレクトリ: `./cruise.yaml` → `./cruise.yml` → `./.cruise.yaml` → `./.cruise.yml`
   - 現在のディレクトリの `./.cruise/*.yaml` / `*.yml`（ファイル名順。最上位の `./.cruise/default.yaml` は除外）
   - `$XDG_CONFIG_HOME/cruise/workflows/*.yaml` / `*.yml` および有効なロック済み bundle directory のメンバー（既定: `~/.config/cruise/workflows/`）。相対パス順。

   stdin と stdout の両方が TTY のとき、候補と組み込み workflow をカテゴリ付きの対話 selector で提示します。pipe 接続 stdin や script などの非対話環境ではプロンプトなしで最優先候補を選びます。
4. **設定ファイルがない場合** — 対話端末では selector に組み込み workflow を提示し、非対話解決ではソースツリーの `builtin/tdd.yaml` にある埋め込み `tdd` workflow を自動選択します。設定ファイルは必須ではありません。

対話 selector は設定された `workflow_categories` ツリーを使います。未設定または空なら、既定の Development (`tdd`、`refactor`)、Quality (`add-tests`、`add-formal-specs`)、Research (`research`)、Security (`security`) を使います。空でない user ツリーは既定値にマージせず置き換えます。カテゴリと workflow 項目は宣言順を保ちます。CLI サブメニューではサブカテゴリを workflow 項目より先に表示します。workflow は複数カテゴリに掲載できます。

修飾なしの項目名はファイルの stem（拡張子を除くファイル名）と組み込み名に一致します。たとえば `refactor` は `refactor.yaml` と組み込み workflow の両方、`builtin:refactor` は組み込みのみと一致します。異なるソースの一致ファイルはすべて表示されます。存在しない名前は無視され、空のカテゴリは省略されます。`{refactor: "Custom description"}` のような項目は selector 説明を指定し、WebUI では workflow の `description:` を上書きします。CLI picker はファイル workflow の description を読み込みません。同じ workflow（組み込みに対する修飾なし名と prefix 付き名を含む）で説明が衝突すると無効です。picker 設定は picker を開くときに検証されますが、実行時の model や parallelism の default 読み込みは妨げません。

未分類のローカル候補と `CRUISE_CONFIG` 候補はルートに残ります（WebUI では `Base dir (...)` の下）。未分類のインストール済み package は `Packages` に並び、その後に未分類の user directory と組み込み workflow が `Others` または設定された `others_category_name` に表示されます。`show_others_category: false` にすると、未分類の user と組み込み候補は非表示になりますが、ローカル、環境変数、インストール済み package の候補は非表示になりません。WebUI では `Auto`、組み込み `tdd`、および他の条件で除外された現在の session workflow も表示されます。カテゴリは対話表示にだけ影響し、明示選択や非対話解決の優先順位は変えません。

<a id="workflow-defaults-defaultyaml"></a>
### ワークフロー既定値 (`default.yaml`)

`$XDG_CONFIG_HOME/cruise/default.yaml` は user default を設定します。`XDG_CONFIG_HOME` 未設定時は macOS/Linux で `~/.config/cruise/default.yaml`、Windows では `APPDATA` 設定時に `%APPDATA%\cruise\default.yaml` が既定です。Windows で `APPDATA` がない場合は home directory の `~/.config/cruise/default.yaml` にフォールバックします。`./.cruise/default.yaml` は project default を設定します。これらのファイルは workflow が未指定の field に既定値を与えます。workflow として選択することはできず、`-c` または `CRUISE_CONFIG` で指定するとエラーになります。`default` は組み込み workflow 名ではないため `cruise workflow eject default` は未知の組み込み名として拒否されます。また `cruise workflow generate --name default` は予約済みの `./.cruise/default.yaml` への書き込みを拒否します。`<config_dir>/workflows/default.yaml` は通常の workflow です。

該当する項目での優先順位（高い順）: invocation ごとの CLI flag（workflow 選択用 `-c` を含む） > 保存済み session cleanup override > 設定環境変数による override > workflow file > `./.cruise/default.yaml` > `$XDG_CONFIG_HOME/cruise/default.yaml` > アプリケーション model default (`config.json` または `config.yaml`) > 組み込み default。通常の prompt では step-level model override が引き続き優先されます。

実際の jcode prompt に有効な model がない場合、Cruise は `$XDG_CONFIG_HOME/cruise/default.yaml`（またはプラットフォーム標準の user config path）に `model: null` を持つ template を作成し、jcode を起動する前に停止します。user-level file を編集して実際の `provider/model[:effort]` reference または bare model を設定してください。既存 file は変更せず、project `.cruise/default.yaml` も生成しません。新しい session では `config.json`、`config.yaml`、workflow、`CRUISE_MODEL` で `model` を設定することもできます。planning call では `plan_model` / `CRUISE_PLAN_MODEL` を使えます。dry-run、draft、listing、command-only 実行、Claude では file は作成されません。

設定後は、最初に処理を始めたコマンドを再試行してください。失敗した run session には `cruise run <SESSION>`、plan や exec の要求にはそれぞれ `cruise plan` または `cruise exec` を再実行します。file-backed session は default を再読み込みしますが、snapshot session は後からの default や環境変更を再適用しません。更新後の設定を使うには新しい snapshot session を開始してください。

- `env` と `mcp_servers` は key ごとにマージし、上位レイヤーが優先されます。
- `command` と `sdk` は一組です。あるレイヤーがどちらかを設定すると、下位レイヤーの組は無視されます。
- その他の field (`model`、`plan_model`、`subagent_model`、`max_retries`、`interactive_planning`、`languages`、`cleanup_after_pr`、`force_exec`、`computer_use`、`permission`、`disallowed_tools`、`retry`、`formal_verify`) は、最上位で設定されたレイヤーの値を使います。制限リストはレイヤー間で置換されます。workflow の `disallowed_tools: []` を明示すると継承した default をクリアします。
- Worktree 設定 `base_branch` と `auto_fetch` は default 専用 key で、project 値が user 値に優先します。マージ後の `base_branch` が空であれば `auto_fetch: true` は無効です。
- `description`、`steps`、`groups`、`after-pr` は workflow 専用であり、`default.yaml` にあると読み込みエラーになります。未知の key もエラーです。
- default が workflow と異なる種類の backend (`command` / `sdk`) を指定している場合、backend 固有の未設定 `model`、`plan_model`、`subagent_model`、`retry` を補う、欠けている `mcp_servers` entry を追加する、または未設定の `computer_use`、`permission`、`disallowed_tools` field を値にかかわらず補うと検証エラーになります。workflow で field を設定するか、`default.yaml` から削除してください。この検査は `CRUISE_SDK` override より前に行われ、`CRUISE_SDK` は考慮されません。

`pre_steps:` は command 専用 step (`command`、`env`、`timeout`、`skip`、`when`) を定義し、workflow の `steps` の先頭に `pre/<name>` として挿入します。planning や `after-pr` では実行されません。user step の後に project step を並べ、同名 project step は user step をその場で置き換えます。

```yaml
# ./.cruise/default.yaml
pre_steps:
  mise-trust:
    command: mise trust || true
  nix:
    command: nix develop --command true
```

更新に関する注意: 組み込み workflow には `mise-trust` step はなくなりました。この動作を維持するには上記の `mise-trust` pre step を追加します。

GitHub Actions では `XDG_*` が分離されるため、project `.cruise/default.yaml` だけが適用されます。

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

テンプレート変数と同じ Rust `format!` 形式の波括弧 escape（[変数リファレンス](steps.md#variable-reference)参照）が使われます。`{{model}}` は placeholder ではなく文字列 `{model}` です。それ以外の未 escape `{name}`、空の `{}`、閉じていない `{`、単独の `}` はテンプレート構文エラーです。

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

`languages.pr` は自動生成する PR title/body の言語、`languages.plan` は cruise 組み込み planning prompt の言語を指定します。`CRUISE_LANGUAGE_PR` と `CRUISE_LANGUAGE_PLAN` はそれぞれ対応する YAML 値を上書きし、空白の環境変数値は無視されます。それ以外では、ネストされた field が非推奨の `pr_language` / `plan_language` より優先され、次に `LC_ALL`、`LC_MESSAGES`、`LANG`、`LANGUAGE` のうち最初の空でない値を1回マッピングして使います（未対応値でも次の変数には進みません）。それもなければ `English` です。非推奨 field は fallback として引き続きサポートされます。

```yaml
languages:
  pr: Japanese             # PR title/body will be generated in Japanese
  plan: Japanese           # generated/updated plans and plan answers will be in Japanese
```

旧設定との互換性:

```yaml
pr_language: Japanese     # Deprecated; use languages.pr instead.
plan_language: Japanese   # Deprecated; use languages.plan instead.
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

`{title}` は trim 済みの LLM 生成 PR title です。生成結果が空なら、session input または title のうち最初の空でない行を trim して使います。`{body}` は trim 済みの LLM 生成 PR body で、生成に失敗すれば空文字列です。`{issue}` は現在常に空文字列、`{session_id}` は session ID、`{branch}` は worktree branch 名です。これらの template は issue を閉じる文言を自動追加しません。

未知の変数と不正な構文は workflow の load/compile 時に拒否されます。文字どおりの波括弧は `{{` と `}}` で escape します。展開後の値は再展開されません。render 後の body と commit message は trim されます。render 結果の commit message が空ならエラーです。設定された Co-authored-by trailer は別に追加されます。PR body template を設定すると、Cruise は `--fill` を使わず title と body を明示的に渡します。

両 template が未設定なら従来の動作は変わりません。commit は生成済み PR title または既存の input fallback を使い、PR title が空の場合は `--fill` を使います。これらの設定は current-branch run、`create_pr: false`、agent が作成する commit、squash-merge message には影響しません。

<a id="session-title-generation"></a>
### session title の生成

plan の承認後、cruise は raw task input の代わりに `cruise list` と WebUI sidebar に表示する、最大80文字の簡潔な session title を生成します。動作は backend によって異なります。

- **SDK mode（`sdk:` が設定されている場合、または適用される `default.yaml` を適用した解決済み workflow に `sdk:` も `command:` も残らない場合 — このときは既定の `jcode` backend）** — foreground `cruise plan` の承認経路では、`plan_model` → `model` の順に使って別の title 生成要求を行います。jcode 呼び出しに有効な model がなければ、承認は失敗し保存されません。それ以外の title 生成エラーでは `plan.md` metadata に fallback します。`cruise list`、WebUI/TUI からの承認と background planning では `plan.md` から title を導出します。
- **command mode（`command:` が設定されている場合）** — title 生成に LLM は呼び出しません。生成済み `plan.md` の最初の見出し、または最初の空でない行から title を導出します。

jcode では、workflow、該当する default、または用途に応じた `CRUISE_MODEL` / `CRUISE_PLAN_MODEL` で有効 model を設定してください。Claude と command workflow は従来どおり model なしでも動作します。

<a id="environment-variables"></a>
### 環境変数

`CRUISE_JCODE_CLI_PATH`、`CRUISE_CLAUDE_CLI_PATH`、`CRUISE_OPENCODE_CLI_PATH` は Cruise process 環境から provider executable を選択します。workflow の設定 key ではなく、workflow `env:` からは指定できません。[管理対象 provider CLI](installation.md#managed-provider-clis) を参照してください。

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

`CRUISE_RUN_ALL_PARALLELISM` は TUI/WebUI run-all 操作と有効設定表示で app 設定を上書きしますが、設定ファイルには保存されません。`--parallelism` を使う CLI `run --all` は変更しません。reload 時、file-backed session は環境 override を再適用し、snapshot session は作成時に解決された設定を保持します。明示的な言語設定がなければ、言語設定は `LC_ALL`、`LC_MESSAGES`、`LANG`、`LANGUAGE` の順で locale 推定に fallback します。

key `env`、`mcp_servers`、`pre_steps` は名前付き entry、command、secret を含むため対象外です。workflow の構造と識別情報 (`steps`、`groups`、`after-pr`、`inputs`、`returns`、`description`、`flags`)、`create_pr` workflow contract、非推奨の `pr_language` / `plan_language` も対象外です。`formal_verify` は workflow YAML または default で設定し、設定環境変数では指定しません。`CRUISE_CONFIG`、`CRUISE_FORGE`、`CRUISE_RUNTIME`、`CRUISE_SKIP_AGENT_AUTH_CHECK`、`CRUISE_DISABLE_NOTIFICATIONS`、`CRUISE_DISABLE_HERDR`、`CRUISE_OBSERVABILITY_ENABLED`、`CRUISE_COMMIT_COAUTHOR_NAME` / `CRUISE_COMMIT_COAUTHOR_EMAIL` などの process switch は設定 override とは別です。

`CRUISE_UI_LANGUAGE` は workflow field や生成 prompt の言語ではなく、process UI の言語を制御します。対応 tag と優先順位は [`cruise config`](cli-reference.md#cruise-config) を参照してください。

`JCODE_OPENAI_SERVICE_TIER` は jcode SDK の OpenAI service tier を制御します。workflow `env:` と process 環境のどちらにも key がない場合、Cruise は `off` を既定値にします。明示値はそのまま使います。

`JCODE_DISABLED_TOOLS` は jcode の comma または改行区切りの無効化 tool list です。`computer_use` が false の場合、Cruise は workflow `env:`、process 環境、private jcode `config.toml` の `[tools].disabled` の順に最初に見つかった list を使い、選択済み policy を捨てずに `macos_computer_use` を追加します。`computer_use` が true で permission が `full`、かつ明示的な web/subagent 拒否がなければ、Cruise はこの環境変数や jcode config を変更しません。そのため jcode 側で無効化された tool は無効なままです。制限付き `permission` mode は選択済み list に対応 tool (`bash`、`read-only` では加えて `edit`、`write`、`apply_patch`、`replace`) を追加します。

`CRUISE_COMMIT_COAUTHOR_NAME` と `CRUISE_COMMIT_COAUTHOR_EMAIL` は、PR 用に cruise が作成する commit に `Co-authored-by:` trailer を加えます。両方とも設定され、空でない必要があります。名前に `<`、`>`、改行が含まれる場合、または address が不正な場合は、commit を失敗させず trailer を無効にします。

`CRUISE_DISABLE_HERDR=1` は上記 herdr lifecycle-state 報告を無効にします。

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
  base_delay_ms: 500        # backoff base (default 500); delay is min(base * 2^(attempt-1), 8s) with jitter
  max_delay_ms: 300000      # waiting cap (default 300000). The computed backoff is already capped at 8s,
                            # so this only binds a server `Retry-After` hint (itself clamped to 60s): a
                            # hinted delay above it moves to the next fallback model, or fails the step
                            # when there is none
  model_fallback: true      # allow switching to a fallback model (default true)
  fallback_chains:          # tried in order: exact "provider/model" (or bare "model") key,
                            # then "provider/*", then "default"
    default:
      - provider-a/model-a
      - provider-b/model-b
    "provider-a/*":
      - provider-c/*
```

SDK retry policy がある場合（`retry:` による指定、または fallback entry を持つ model array による暗黙指定のいずれでも）、429 は `Retry-After` を待たず、次に利用できる fallback model へ直ちに切り替わります。使える fallback が残っていない場合に限り同じ model を再試行します。HTTP 5xx、429 以外の HTTP 4xx、401、403、407、408、network failure も retry 対象になり、`--rate-limit-retries` が0より大きく、利用可能な fallback があり、表示済みテキストがなく、fresh budget と fresh session が使える場合は、次の chain entry に即座に切り替えます。そのような 4xx は切替のみで応答します。同じ request を同じ model に再送することはありません。なぜなら同一 request は同じ失敗をするからです。エラー文が恒久的に見える (`invalid request`、`context length`、`max_tokens`) 場合でも、別 model が同じ request を受け付ける可能性があるため 400 は対象です。provider 自身の error に、子 process の stderr から後付けされたものではなく、不在 model を表す名前（`model_not_supported`、`model_not_found`、`model not found`、`model not supported`、`model is not supported`、`unknown model`、`no such model`、`model does not exist`、`not a valid model`）があれば、代わりに missing model と分類します。この場合も切替のみですが、原因は request ではなく model reference なので、離れた model は cooldown になります。その例では `invalid_request_error: model not found` は permanent failure ではなく切替です。使える chain entry がなくなれば、元の 4xx をそのまま返します。除外 status は独自の扱いです。401、403、407 は他 model では解決できない認証／permission error なので、chain をたどって隠さず provider 自身の error で直ちに step を失敗させます。408 Request Timeout は network failure と分類されるため、5xx/network と同じ経路を使います。利用可能な fallback へ先に切り替え、同じ model の再送は利用可能な fallback がない、表示済みテキストがある、または `--rate-limit-retries` が `0` の場合に限ります。

分類は rate-limit 文言を最初に確認し、次に 5xx と network marker、次に missing-model 表現を bare 4xx らしい数字より先に確認します。そのため stderr に追記された stack trace や URL などに 4xx らしい値があっても、`429` や `503` は本来の分類を保ちます。認識可能な status code のない失敗（例: `unknown option '--effort'`）は permanent のまま直ちに step を失敗させます。数字を HTTP status と判定するのは、独立した3桁の数字の前24 byte以内に `http`、`status`、`code`、`error`、`returned`、`upstream` のいずれかがある場合だけです。ソース位置 (`src/lib.rs:404:17`) や URL port (`https://host:443/path`) は status として扱いません。`provider/*` chain entry は失敗した model ID を維持し、provider だけを変更します。

`--rate-limit-retries 0` は retry を無効にします。rate limit、5xx、4xx は model を切り替えずに step を失敗させます。ただし backend が model reference 自体を明示的に拒否した場合は、何も送信されておらず再生も不要なため、次の chain entry に進みます。429、5xx、network failure（408 を含む）でスキップされた model は、現在の process 内で次の30分間スキップ状態となります（メモリー上の状態であり process をまたいで保存されません）。provider が存在しないと報告した model も同様です。後続 turn/step で provider が持たない model を再選択しないためです。4xx client-error によって離れた model だけは cooldown されません。client error は request についての情報であり model の健全性を示さないためです。表示済みテキストがある turn は、別 model で再試行されません。利用可能な fallback がない429では、計算された待機時間または server 要求待機時間が `retry.max_delay_ms` を超えると、同一 model retry を省略できます。

認識される network failure には request 送信時の transport error が含まれます。prompt 受理前の OpenCode setup timeout もこの retry/fallback 経路を使います。

prompt 送信前の OpenCode local V2 API error が rate-limit、client、server failure に分類される場合は retry barrier の対象であり、network-class setup/transport failure だけが fallback のために barrier を迂回します。

`disabled_during` で宣言された startup 除外は response retry budget と独立しています。`--rate-limit-retries 0` でも適用されます。

`retry:` を宣言すると、`model_fallback: false` や空の chain も含め、`retry:` なしの動作が変わります。scalar model 設定ではこれらは model 切替だけを無効にし、4xx/5xx/network の分類と `base_delay_ms` / 8秒上限の backoff schedule は引き続き有効です。fallback entry を持つ workflow-level model array は常に切替を有効にし、fallback chain を生成します。従来の動作（limit error のみ、同じ model、2秒から始まり最大60秒まで倍増）を維持するには、block 自体を省略し、model field を scalar または未設定にしてください。`command:` backend は常に従来の動作を使い、`retry:` を無視します。
