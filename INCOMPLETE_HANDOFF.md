# Incomplete Handoff: PR #459 CI Fix

## Status
PR #459 の失敗していた CI (`test` / `test-tauri`) の原因を修正し、修正内容を PR ブランチに push 済みです。ローカルでは該当箇所が解消していることを確認しました。実際の GitHub Actions の再実行結果は未確認です。

## Done (this session)
- `src/engine.rs`
  - `run_config_inner` 内で `build_dag` のエラーを `panic!` ではなく `?` で伝播するように変更
  - これにより `engine::tests::test_next_pointing_to_nonexistent_step` がパスするようになった
- `src-tauri/src/commands.rs`
  - 存在しなくなった `cruise::engine::execute_steps` の参照を除去
  - Tauri GUI 側も `execute_steps_with_dag` を使うように移行
  - セッションの `current_step` / `current_step_is_node_id` / `has_dag` を `on_node_start` コールバックで更新するようにした
  - `build_dag` のエラーを IPC エラーとして返せるようにした
- `cargo fmt` を実行（`src/engine.rs` / `src/worktree_pr.rs` にもフォーマット変更が入った）
- 修正をコミット・push
  - commit: `555889c`
  - branch: `cruise/20260628172537949_58fda4af44a44f249125b6fec6f3e4ff-DAG-execute-steps-Requirements`

## Verification (local)
- `cargo test --lib engine::tests::test_next_pointing_to_nonexistent_step` → pass
- `cargo check --manifest-path src-tauri/Cargo.toml` → pass
- `cargo fmt -- --check` → pass
- `cargo test --all-features` → lib テストは全て pass、bin テストで `run_cmd::tests::*` がローカル環境でタイムアウトするものがあるが、stash した元のコードでも同様に失敗するため既存の環境依存のフレーキーと判断

## Remaining
1. GitHub Actions の結果確認
   - `gh pr checks 459` で `test (macos-latest)` / `test (ubuntu-latest)` / `test-tauri` が pass するか確認
   - まだ失敗している場合はそのログを確認して追加修正
2. 必要に応じて他の CI ジョブ（`plan` / リリース関連）も確認

## Next-Agent Starting Position
- ブランチはすでに push 済み: `cruise/20260628172537949_58fda4af44a44f249125b6fec6f3e4ff-DAG-execute-steps-Requirements`
- まず `gh pr checks 459` を実行して CI 状況を確認
- 失敗が残っている場合は `gh run view <run-id> --log-failed` で詳細を確認し、本リポジトリと同様の手順で修正・push する
