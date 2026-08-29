# PROHIBITED.md — jcode/ClaudeSDK 移行における禁止事項

[JCODE.md](JCODE.md) の全フェーズに適用する。違反する変更はレビューで差し戻す。

## 1. 設定ファイル形式の変更禁止

- `cruise.yaml` の **YAML 構造・既存フィールド名・既存フィールドの型と意味を変更しない**。
  - 許可される差分は次の 3 つだけ:
    1. `sdk` の受理値変更（`seher` / `pi` 削除、`jcode` / `claude` 追加）
    2. **optional な新フィールドの追加**（`retry:` ブロック）— 未指定時は従来挙動
    3. `sdk`/`command` **両未指定**（現在は "either `command` or `sdk` must be specified" の validation error）を `sdk: jcode` 相当の既定として valid 化する — 現在 valid な設定の挙動は一切変えない
  - `command` / `model` / `plan_model` / per-step `model` / `env` / `max_retries` / `interactive_planning` / steps スキーマ等の意味変更・改名・必須化は禁止。
- `cruise-schema.json` は `sdk` enum と additive な `retry` 定義以外を変更しない。
- `~/.config/seher/config.yaml` 形式の設定ファイルを cruise に持ち込まない。新規の cruise 専用グローバル設定ファイルを発明しない。
- `CRUISE_SDK` 等の既存環境変数の意味を変更しない。
- jcode の設定形式（`config.toml` / `mcp.json`）や Claude Code の設定形式を独自拡張しない — 各ツールが定義するスキーマのみ使用。

## 2. 呼び出し面（API 形状）の破壊禁止

- `Executor::new(sdk, command)` / `PromptRun` / `PromptOutcome` のフィールド構成・セマンティクスを変えない（`tools` の要素型の置換のみ可）。
- 呼び出し元 `src/engine.rs` / `src/planning.rs` / `src/plan_cmd.rs` / `src/worktree_pr.rs` の実質的改変禁止（型名追随・コメント修正のみ可）。`src-tauri/**` は Executor を直接呼ばないが、`cruise::planning::read_sdk_transcript` / `sdk_plan_tools_enabled` 経由で依存しており、この 2 関数のシグネチャと挙動を壊さない。
- `src/sdk_tools.rs` の 6 ツールの名前・JSON スキーマ・handler の挙動・エラー文言を変更しない。
- `command:` バックエンド（`src/step/**`）の挙動を変更しない。

## 3. seher の残留・偽装禁止

- 移行完了時点で `seher-sdk` 依存・`seher`/`pi` の sdk 値・mode_key 概念（`build`/`plan`）・`~/.config/seher` 参照・`codexbar` 連携が **コード/設定/スキーマに一切残らない** こと。
- `sdk: seher` を `jcode` 等へ黙って読み替える互換 alias・shim・deprecated パスを作らない。旧値は明確な validation error（移行案内つき）で拒否する。

## 4. jcode 本体のフォーク禁止

- jcode のソース改変・フォークビルドを行わない。ユーザインストール済みの公式 `jcode` バイナリを使う。
- custom tools は **stdio MCP ブリッジ経由のみ**。jcode 内部（`Registry` 等）への注入を試みない。
- （案 A fallback 時に）vendor する `jcode-sdk` / `jcode-harness-api` / `jcode-transport` はコピー元コミット SHA を記録し、機能追加・挙動変更を加えない（`#[allow]` 追加等のビルド調整のみ可）。crates.io publish を壊さないため、path 直依存ではなく自名義 publish + `[patch.crates-io]` で参照する。

## 5. claude-agent-sdk の改変最小化とライセンス遵守

- `vendor/claude-agent-sdk/` はコピー元（crates.io published `seher-claude-agent-sdk` 0.0.59, Apache-2.0）から**ソース無改変**で展開し、`[patch.crates-io]` で参照する（workspace member 化・path 直依存で crates.io publish を壊さない）。バグ修正が必要な場合も glue（cruise 側）で吸収できないか先に検討する。
- Apache-2.0 の LICENSE ファイル・著作権表示を vendor ディレクトリに保持する（.crate に LICENSE が含まれない場合は `../seher/LICENSE` をコピーして同梱）。削除・MIT への差し替え禁止。
- vendor した jcode 系 crate の MIT ライセンス表示（Copyright (c) 2025 Jeremy Huang）も同様に保持。

## 6. ユーザ環境・対象リポジトリの汚染禁止

- ユーザの `~/.jcode`（config.toml / 認証 / セッション / mcp.json）に読み書きとも触らない。cruise の MCP 登録・認証・セッションは cruise 管理の専用 `JCODE_HOME` 配下のみ。
- ワークフロー対象リポジトリ（worktree 含む）に `.jcode/` / `.mcp.json` / `.claude/mcp.json` 等の作業ファイルを書き込まない・残さない。
- 対象リポジトリの project-local MCP 設定（`.mcp.json` / `.jcode/mcp.json` / `.claude/mcp.json`）は jcode の後勝ちマージで cruise のセッションに load され得る。無警告でこれを許容しない — launch 前に検知して警告し、server 名 `cruise` の衝突は明確なエラーにする。
- jcode をテレメトリ有効のまま埋め込み起動しない（`JCODE_NO_TELEMETRY=1` 必須）。自動アップデートを cruise 経由で走らせない。
- **認証分離**: jcode の起動は `inherit_logins: false` 固定。ユーザの `~/.jcode` の認証情報（OAuth / `<provider>.env`）を読み取り・コピー・継承しない。cruise 用認証は cruise 専用 `JCODE_HOME` 配下のみに保存する。
- ユーザの認証情報・API キーをログ・設定ファイル・エラーメッセージに出力しない。cruise の設定ファイル（cruise.yaml 等）に API キーを保存するフィールドを追加しない。

## 7. フォールバック実装の禁止事項

- `retry:` 未設定時の挙動を変えない: 従来どおり同一モデルへの再試行のみ（fallback チェーンは opt-in）。
- リトライ回数の出所は既存 `--rate-limit-retries` / `PromptRun::max_retries` のまま。並行する別のリトライ回数設定を追加しない。
- 可視出力（text delta）をユーザへストリーム済みのターンを黙って別モデルで再実行しない（リプレイ安全性）。
- 部分応答済みセッションへ同一プロンプトを再送しない（コンテキスト重複）。再試行は常に fresh session。
- 429 以外の恒久エラー（認証失敗・invalid_request・コンテキスト超過）をリトライ対象に分類しない。

## 8. プロセス上の禁止事項

- 既存テストの削除・`#[ignore]` 化・アサーション弱体化で通過させない。`"seher"` 文字列を含むテストは検証意図を保ったまま新値へ更新する。
- フェーズ途中でビルド不能・テスト不通過の状態をコミットしない（各フェーズ green 維持）。
- スタブ・`todo!()`・未配線コードを「後続フェーズで実装」名目で main に残さない。
- 本移行スコープ外のリファクタリング・機能追加（「ついで」の抽象化・依存更新）を混ぜない。
