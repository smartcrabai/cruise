# Incomplete Handoff: languages.pr / languages.plan 設定対応

## Status
コア実装とテスト、JSON Schema 更新は完了。残りはドキュメント更新と最終検証（clippy / GUI チェック）。

## Done
- `src/config.rs`
  - `LanguagesConfig` struct (`pr`, `plan`) を追加
  - `WorkflowConfig.pr_language` / `plan_language` を `Option<String>` に変更
  - `WorkflowConfig.languages: Option<LanguagesConfig>` を追加
  - `effective_pr_language()` / `effective_plan_language()` を追加（trim + 空文字 fallback）
  - `deprecated_language_warnings()` を追加（Vec<String> 返却）
  - `default_builtin()` の言語フィールドを `None` に変更
  - 既存テストを `Option<String>` / effective メソッドに対応させ更新
  - 新規テストを追加: `languages` キー、precedence、後方互換、警告有無、blank fallback
  - スキーマテスト `test_schema_workflow_config_has_expected_properties` に `"languages"` を追加
- `src/workflow.rs`
  - `compile()` で `config.effective_pr_language()` / `config.effective_plan_language()` を使用
- `src/planning.rs`
  - `setup_plan_vars` の `config.plan_language.trim()` を `config.effective_plan_language()` に置き換え
- `src/worktree_pr.rs`
  - `build_pr_prompt` の `compiled.pr_language.trim()` を削除（解決済み値をそのまま使用）
- `src/workflow_call.rs`
  - `resolve_workflow_calls_from_path` で `deprecated_language_warnings()` を取得し `eprintln!("warning: {msg}")` で出力
  - 既存テストの `config.pr_language` / `config.plan_language` 比較を `Option<String>` に対応
- `cruise-schema.json`
  - `languages` プロパティを追加（`LanguagesConfig` 相当）
  - `pr_language` / `plan_language` の description に deprecated 注記を追加
- `cargo test` 全 885 件パス

## Remaining
1. ドキュメント更新
   - `README.md`: Basic Structure サンプル、PR Language / Plan Language 節を `languages` ベースに更新し、旧キーを deprecated 注記付きで残す
   - `skills/cruise-config/references/top-level.md`: 同様に更新
   - `skills/cruise-config/SKILL.md`: 目次の `pr_language` 表記を `languages` に更新

2. 最終検証
   - `cargo clippy -- -D warnings` で警告ゼロ
   - `cargo check -p cruise-tauri` / `cargo check -p cruise-gui` で GUI 側に影響なしを確認

## Next-Agent Starting Position
- 現在のコミット: `861fcf9` (`WIP: implement languages.pr/languages.plan with legacy deprecation warnings`)
- ブランチ: `cruise/20260622065653266-pr-language-plan-language-lang`
- 次は `README.md` と `skills/cruise-config/` のドキュメント更新から着手
- ドキュメント更新後は `cargo clippy -- -D warnings` と GUI チェックで仕上げ

## Plan Reference
`/Users/takumi/.local/share/cruise/sessions/20260622065653266/plan.md`
