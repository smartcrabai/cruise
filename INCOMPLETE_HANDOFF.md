# Incomplete Handoff: GUI 版 DAG 描画機能

## Status
`/write-test-first` により計画書に基づくテストファースト実装を開始。バックエンドの DTO と `get_session_dag` コマンドの骨格、並びに Tauri ハンドラ登録まで完了。フロントエンドの型・コマンドラッパー・コンポーネント・タブ統合、および各種テストの追加は未着手。

## Done (this session)
- 計画書 `/Users/takumi/.local/share/cruise/sessions/20260629065146988_fb6aecb7b2894d64b99b11d310e16eb5/plan.md` を読み込み、実装方針を確認
- 関連コードを調査
  - `src/dag.rs` (`ExecutionDag`, `DagNode`, `NodeSuccessor`, `TransitionReason`)
  - `src/workflow.rs` (`CompiledWorkflow`, `compile`)
  - `src-tauri/src/commands.rs` (`get_session_plan`, `SessionDto`, テストモジュール)
  - `src-tauri/src/lib.rs` (`invoke_handler`)
  - `ui/src/App.tsx` (`ActiveTab`, `WorkflowRunner`, タブボタン/パネル)
  - `ui/src/types.ts`, `ui/src/lib/commands.ts`, `ui/src/components/WorkflowPlanPanel.tsx` およびテスト類
- `src-tauri/src/commands.rs` に DAG 可視化用 DTO を追加
  - `DagDto`, `DagStepDto`, `DagEdgeDto`
- `src-tauri/src/commands.rs` に集約関数 `build_dag_dto` のスタブを追加
- `src-tauri/src/commands.rs` に `get_session_dag` コマンドを追加（`load_config` → `compile` → `build_dag` → `build_dag_dto` の流れ）
- `src-tauri/src/lib.rs` の `invoke_handler!` に `commands::get_session_dag` を登録
- 作業をコミット
  - commit: `7111c6e`
  - message: `wip(dag): add backend DTOs, stub get_session_dag command, register handler`

## Remaining
1. **バックエンド集約ロジックの実装**
   - `src-tauri/src/commands.rs` の `build_dag_dto` スタブを実装
     - `ExecutionDag.nodes` を step 名で集約
     - 同一 `(from_step, to_step, reason, selector)` のエッジを dedupe
     - 各 step の kind を `CompiledWorkflow.steps` から判定（prompt / command / option）
     - `is_terminal` は `target == None` のエッジが存在するかで判定
     - `current_step` は `current_step_is_node_id` を考慮して step 名に解決
   - `src-tauri/src/commands.rs` のテストモジュールに `build_dag_dto` のテストを追加
     - 線形 workflow
     - option 分岐
     - `if.file-changed` 分岐
     - `if.fail` retry
     - group retry
     - `current_step` の node id / step 名解決

2. **フロントエンド型と API ラッパー**
   - `ui/src/types.ts` に `DagDto`, `DagStepDto`, `DagEdgeDto` を追加
   - `ui/src/lib/commands.ts` に `getSessionDag(sessionId)` を追加

3. **描画コンポーネントの実装**
   - `ui/package.json` に `mermaid` 依存を追加
   - `ui/src/components/WorkflowDagPanel.tsx` を新規作成
     - `getSessionDag` を dynamic import せず通常 import で呼び出し
     - `mermaid` は dynamic import (`await import("mermaid")`)
     - `buildMermaidSource` ヘルパーで Mermaid 構文を生成
       - step 名を Mermaid ID に sanitize（連番 prefix + 非英数字置換）
       - 終端エッジは共通 `end[/END/]` ノードへ
       - `currentStep` は青色スタイルでハイライト
     - ローディング / エラー / SVG 表示の状態管理
     - セッション切替時の古いレンダリング破棄（`renderId` カウンター）

4. **App.tsx へのタブ統合**
   - `ActiveTab` に `"dag"` を追加
   - タブボタンとパネルを Info と Plan の間に配置
   - `WorkflowDagPanel` をレンダリング

5. **テストの追加**
   - `ui/src/components/WorkflowDagPanel.test.tsx`
     - loading 表示
     - `getSessionDag` 呼び出し
     - Mermaid SVG レンダリング
     - エラー表示
     - セッション切替時の再取得
     - `buildMermaidSource` の出力検証（node id sanitize, 終端エッジ, current step ハイライト）
   - `ui/src/__tests__/WorkflowRunner.test.tsx`
     - mock に `getSessionDag` を追加
     - DAG タブが表示されること
     - DAG タブ選択時に `WorkflowDagPanel` がマウントされ `getSessionDag` が呼ばれること
   - 必要に応じて `App.test.tsx` 系のタブ数アサーションを修正

6. **検証・ビルド**
   - `cargo test -p cruise-gui`（または `cargo test --package cruise-gui`）
   - `pnpm --dir ui test`
   - `pnpm --dir ui lint`
   - `pnpm --dir ui build`
   - `cargo clippy -p cruise-gui --all-targets`

7. **手動動作確認と PR 作成**
   - `pnpm --dir ui dev` / `cargo tauri dev` で GUI 起動
   - 既存セッションの DAG タブを開いて描画確認
   - 分岐 workflow でのエッジ描画確認
   - スクリーンショット撮影して PR body に添付
   - PR 作成前に `/review-all` を実行

## Next-Agent Starting Position
- ブランチ: `cruise/20260629065146988_fb6aecb7b2894d64b99b11d310e16eb5-GUI-DAG-PR-DAG`
- コミット `7111c6e` までの変更はコミット済み
- まず `src-tauri/src/commands.rs` の `build_dag_dto` スタブを実装し、テストを追加・実行
- 次にフロントエンドの型/API/コンポーネント/App.tsx 統合を順に実装
- 各ステップで計画書を参照し、計画書の未確定事項（step 単位表示で良いか、タブ配置位置）に留意
