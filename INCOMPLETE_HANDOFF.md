# Incomplete Handoff: languages.pr / languages.plan 設定対応

## Status
`src/config.rs` のデータモデルと解決ヘルパー、警告ロジック、単体テストを追加済み。
残りは他ファイルへの影響箇所の修正とドキュメント更新。

## Done
- `src/config.rs`
  - `LanguagesConfig` struct (`pr`, `plan`) を追加
  - `WorkflowConfig.pr_language` / `plan_language` を `Option<String>` に変更
  - `WorkflowConfig.languages: Option<LanguagesConfig>` を追加
  - `effective_pr_language()` / `effective_plan_language()` を追加（trim + 空文字 fallback）
  - `deprecated_language_warnings()` を追加（Vec<String> 返却、eprintln は呼び出し側で）
  - `default_builtin()` の言語フィールドを `None` に変更
  - 既存テストを `Option<String>` / effective メソッドに対応させ更新
  - 新規テストを追加: `languages` キー、precedence、後方互換、警告有無、blank fallback

## Remaining
1. `src/workflow.rs`
   - `compile()` で `config.effective_pr_language()` / `config.effective_plan_language()` を使用
   - `CompiledWorkflow.pr_language` / `plan_language` は String のまま（解決済み値）
   - 既存テストは基本的にそのまま、新キーを使うテストも追加推奨

2. `src/planning.rs`
   - `setup_plan_vars` の `config.plan_language.trim()` を `config.effective_plan_language()` に置き換え

3. `src/worktree_pr.rs`
   - `build_pr_prompt` の `compiled.pr_language.trim()` を削除（`compiled.pr_language` は既に解決済み）

4. `src/workflow_call.rs`
   - `resolve_workflow_calls_from_path` の戻り値に対し `deprecated_language_warnings()` を呼び出し、各メッセージを `eprintln!("warning: {msg}")` で出力
   - テスト `test_resolve_workflow_call_ignores_callee_top_level_execution_settings` で `config.pr_language` / `config.plan_language` が `Option<String>` になったので比較を修正

5. `src/plan_cmd.rs` / `src/run_cmd.rs`
   - テストで直接 `pr_language` / `plan_language` を YAML から指定している箇所は、新キー版テストを追加または既存テストを effective メソッドベースに更新
   - blank fallback テストは `effective_*_language()` 側で処理されるため呼び出し元の `.trim()` は不要になる

6. `cruise-schema.json`
   - `languages` プロパティを追加（`LanguagesConfig` 相当）
   - `pr_language` / `plan_language` の description に deprecated 注記を追加

7. `src/config.rs` スキーマテスト
   - `test_schema_workflow_config_has_expected_properties` の期待フィールドに `"languages"` を追加

8. ドキュメント
   - `README.md`: Basic Structure サンプル、PR Language / Plan Language 節を `languages` ベースに更新し、旧キーを deprecated 注記付きで残す
   - `skills/cruise-config/references/top-level.md`: 同様に更新
   - `skills/cruise-config/SKILL.md`: 目次の `pr_language` 表記を `languages` に更新

9. 検証
   - `cargo test` 全パス
   - `cargo clippy -- -D warnings` で警告ゼロ
   - `cargo check -p cruise-tauri` / `cargo check -p cruise-gui` で GUI 側に影響なしを確認

## Next-Agent Starting Position
- 現在のコミット: `02965a5` (`WIP: add LanguagesConfig and effective_*_language helpers (config.rs)`)
- ブランチ: `cruise/20260622065653266-pr-language-plan-language-lang`
- 次は `src/workflow.rs` の `compile()` から修正し、コンパイルエラーを潰しつつテストを追加していくのが順調
- `src/config.rs` の変更が中心なので、まず `cargo test --lib config` や `cargo check` を実行して型エラーを確認するとよい

## Plan Reference
`/Users/takumi/.local/share/cruise/sessions/20260622065653266/plan.md`
