# Incomplete Handoff: DAG 駆動実行への完全移行

## Status
`src/engine.rs` の `execute_steps_with_dag` 整理を開始。残りは call site 更新、run_cmd / worktree_pr / session 統合、テスト追加・修正、最終検証。

## Done
- `src/engine.rs`
  - `execute_steps_with_dag` の重複していた `config_reloader` ブロックを整理
  - `on_node_start` コールバックのシグネチャを `Fn(&NodeCheckpoint, &ExecutionDag)` に変更し、呼び出し側で DAG を永続化できるようにした
  - 関数内のコメントを更新
- 現在の作業をコミット: `3d0d86b` (`WIP: DAG execution integration (engine on_node_start signature + reloader cleanup)`)

## Remaining
1. `src/engine.rs` 内の `execute_steps_with_dag` 呼び出し箇所を新シグネチャに合わせて修正（テスト含む `&|_| Ok(())` → `&|_, _| Ok(())` など）
2. `src/session.rs`
   - `dag_path()` ヘルパーを追加
3. `src/run_cmd.rs`
   - import を `execute_steps_with_dag, NodeCheckpoint` に変更
   - `load_or_build_session_dag()` / `resolve_start_node()` を追加
   - `run_single` で DAG をビルド/ロードし、node id で再開する経路に切り替え
   - `on_node_start` コールバックで `current_step` に node id を保存し、`dag.json` も永続化
   - 実行終了時にも `dag.json` を保存
   - `log_resume_message` を node id ではなく step name を表示するよう更新
4. `src/worktree_pr.rs`
   - import を更新
   - `run_after_pr_steps` を DAG 経由（`build_dag` → `execute_steps_with_dag`）に書き換え
5. `src/dag.rs`
   - 先頭の `#![allow(dead_code)]` を削除して未使用警告を健全化
6. テスト修正・追加
   - `src/engine.rs` の既存 `execute_steps_with_dag` 呼び出しを新シグネチャに更新
   - mock ベース停止→再開 E2E テスト追加（linear、retry budget、dag.json round trip）
   - `src/run_cmd.rs` の resume 系テストを node id ベースに更新
   - 旧セッションの auto-migration テスト `test_run_migrates_legacy_session_without_dag` を追加
7. 最終検証
   - `cargo test --workspace`
   - `cargo clippy --all-targets`
   - `rg "execute_steps\\b" src` で旧関数参照がゼロ件であることを確認

## Next-Agent Starting Position
- 現在のコミット: `3d0d86b` (`WIP: DAG execution integration (engine on_node_start signature + reloader cleanup)`)
- ブランチ: `cruise/20260628172537949_58fda4af44a44f249125b6fec6f3e4ff-DAG-execute-steps-Requirements`
- まず `src/engine.rs` 内の `execute_steps_with_dag` 呼び出しを新シグネチャ `|cp, dag|` に修正してコンパイルエラーを解消し、その後 `src/run_cmd.rs` / `src/worktree_pr.rs` / `src/session.rs` の統合に進む

## Plan Reference
`/Users/takumi/.local/share/cruise/sessions/20260628172537949_58fda4af44a44f249125b6fec6f3e4ff/plan.md`
