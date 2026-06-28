# Incomplete Handoff: DAG 駆動実行への完全移行

## Status
CI エラー修正を開始。`src/engine.rs` / `src/worktree_pr.rs` / `src/run_cmd.rs` のコンパイルエラーを解消するため、最小限の DAG 移行を実施中。最終検証は未完了。

## Done
- `src/engine.rs`
  - `execute_steps_with_dag` 内の重複・壊れた `on_node_start` 呼び出しを修正
  - `on_node_start` コールバック呼び出しを新シグネチャ `Fn(&NodeCheckpoint, &ExecutionDag)` に合わせて更新
  - テスト内の `execute_steps_with_dag` コールバックを `|_cp, _dag|` / `|cp, _dag|` に更新
  - `new_dag` の move-after-borrow エラーを `clone()` で回避
  - 重複していた `LoopState` の doc comment を整理
- `src/worktree_pr.rs`
  - `execute_steps` の import を `execute_steps_with_dag, NodeCheckpoint` に変更
  - `run_after_pr_steps` を DAG 経由（`build_dag` → `execute_steps_with_dag`）に書き換え
- `src/run_cmd.rs`
  - `execute_steps` の import を `execute_steps_with_dag, NodeCheckpoint` に変更
  - `run_single` で DAG をビルドし、`current_step_is_node_id` / step name から再開 node id を解決する経路を追加
  - `on_step_start` コールバックで `current_step` に node id を保存し、`current_step_is_node_id = true`、`has_dag = true` を設定
  - `execute_steps` 呼び出しを `execute_steps_with_dag` に切り替え
- 現在の作業をコミット: `87b19a4` (`WIP: fix CI compilation errors - engine/worktree_pr/run_cmd DAG migration (incomplete)`)

## Remaining
1. `src/run_cmd.rs`
   - `log_resume_message` を node id ではなく step name を表示するよう更新（現在は未変更のため、node id が表示される可能性あり）
   - `load_or_build_session_dag()` / `resolve_start_node()` ヘルパーの整理（任意）
   - 実行終了時にも `dag.json` を保存する永続化対応
   - 旧セッション（`has_dag=false` かつ step name のみ保存）の auto-migration テスト追加
2. `src/session.rs`
   - `dag_path()` ヘルパーを追加（`run_cmd.rs` から `dag.json` 保存に使用）
3. `src/dag.rs`
   - 先頭の `#![allow(dead_code)]` を削除して未使用警告を健全化
4. テスト修正・追加
   - mock ベース停止→再開 E2E テスト追加（linear、retry budget、dag.json round trip）
   - `src/run_cmd.rs` の resume 系テストを node id ベースに更新
   - 旧セッションの auto-migration テスト `test_run_migrates_legacy_session_without_dag` を追加
5. 最終検証
   - `cargo test --workspace`
   - `cargo clippy --all-targets`
   - `rg "execute_steps\b" src` で旧関数参照がゼロ件であることを確認

## Next-Agent Starting Position
- 現在のコミット: `87b19a4` (`WIP: fix CI compilation errors - engine/worktree_pr/run_cmd DAG migration (incomplete)`)
- ブランチ: `cruise/20260628172537949_58fda4af44a44f249125b6fec6f3e4ff-DAG-execute-steps-Requirements`
- まず `cargo check` / `cargo clippy` を実行し、残っているコンパイルエラー・警告を確認
- `log_resume_message` の step name 表示対応と、`dag.json` 永続化を `src/session.rs` の `dag_path()` 追加と合わせて実装
- その後テスト追加・修正と最終検証に進む
