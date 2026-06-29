# Incomplete Handoff: GUI 版 DAG 描画機能

## Status
`/implement-after-tests` により計画書に基づく実装を開始。バックエンドの `build_dag_dto` 実装と Tauri ハンドラ登録の修正、フロントエンドの型・コマンドラッパー・mermaid 依存・`WorkflowDagPanel` コンポーネント作成まで完了。`App.tsx` へのタブ統合とテスト追加は未着手。ツールイテレーション制限のためここでハンドオフ。

## Done (this session)
- 計画書 `/Users/takumi/.local/share/cruise/sessions/20260629065146988_fb6aecb7b2894d64b99b11d310e16eb5/plan.md` を読み込み、実装方針を確認
- 関連コードを調査
  - `src/dag.rs` (`ExecutionDag`, `DagNode`, `NodeSuccessor`, `TransitionReason`)
  - `src/workflow.rs` (`CompiledWorkflow`, `compile`)
  - `src-tauri/src/commands.rs` (`get_session_plan`, `SessionDto`)
  - `src-tauri/src/lib.rs` (`invoke_handler`)
  - `ui/src/App.tsx` (`ActiveTab`, `WorkflowRunner`, タブボタン/パネル)
  - `ui/src/types.ts`, `ui/src/lib/commands.ts`, `ui/src/components/WorkflowPlanPanel.tsx` およびテスト類
- `src-tauri/src/commands.rs`
  - `build_dag_dto` をスタブから本実装へ変更
    - `ExecutionDag.nodes` を step 名で集約
    - 同一 `(from_step, to_step, reason, selector)` のエッジを dedupe
    - 各 step の kind を `CompiledWorkflow.steps` から判定（prompt / command / option / unknown）
    - `is_terminal` は `target == None` のエッジが存在するかで判定
    - `current_step` は `current_step_is_node_id` を考慮して step 名に解決
  - `step_kind`, `transition_reason` ヘルパーを追加
- `src-tauri/src/lib.rs`
  - `invoke_handler!` における重複した `commands::get_session_plan` を削除（`get_session_dag` は既に登録済み）
- `cargo check -p cruise-gui` でバックエンドがコンパイルすることを確認
- フロントエンド型/API
  - `ui/src/types.ts` に `DagDto`, `DagStepDto`, `DagEdgeDto` を追加
  - `ui/src/lib/commands.ts` に `getSessionDag(sessionId)` を追加
- `ui/package.json` に `mermaid` 依存を追加（v11.16.0）
- `ui/src/components/WorkflowDagPanel.tsx` を新規作成
  - `getSessionDag` を呼び出して DAG データを取得
  - `mermaid` は dynamic import (`await import("mermaid")`)
  - `buildMermaidSource` ヘルパーで Mermaid 構文を生成
    - step 名を Mermaid ID に sanitize（連番 prefix + 非英数字置換）
    - 終端エッジは共通 `end[/END/]` ノードへ
    - `currentStep` は青色スタイルでハイライト
    - `startStep` は緑色スタイルでハイライト
  - ローディング / エラー / SVG 表示の状態管理
  - セッション切替時の古いレンダリング破棄（`renderId` カウンター）
- 作業をコミット
  - commit: `b27aa59`
  - message: `WIP: implement DAG backend and start frontend panel`

## Remaining
1. **App.tsx へのタブ統合**
   - `ActiveTab` に `"dag"` を追加（`ui/src/App.tsx:465`）
   - `WorkflowRunner` 内で `tabDagId` / `panelDagId` を `useId` から派生して追加
   - タブボタン（`role="tab"`）を Info と Plan の間に配置
     - `id={tabDagId}`, `aria-selected={activeTab === "dag"}`, `aria-controls={panelDagId}`, `onClick={() => onActiveTabChange("dag")}`
   - パネル切替に `activeTab === "dag" && <WorkflowDagPanel sessionId={session.id} panelId={panelDagId} tabId={tabDagId} />` を追加
   - `WorkflowDagPanel` の import を `ui/src/App.tsx` 先頭に追加

2. **テストの追加・修正**
   - `ui/src/components/WorkflowDagPanel.test.tsx` を新規作成
     - loading 表示
     - `getSessionDag` 呼び出し
     - Mermaid SVG レンダリング（mermaid を mock 化）
     - エラー表示
     - セッション切替時の再取得
     - `buildMermaidSource` の出力検証（node id sanitize, 終端エッジ, current step ハイライト）
   - `ui/src/__tests__/WorkflowRunner.test.tsx`
     - mock に `getSessionDag` を追加
     - DAG タブが表示されること
     - DAG タブ選択時に `WorkflowDagPanel` がマウントされ `getSessionDag` が呼ばれること
   - 既存 `ui/src/test/App.*.test.tsx` 系でタブ数や順序を前提にしたアサーションがあれば修正

3. **検証・ビルド**
   - `cargo test -p cruise-gui`（または `cargo test --package cruise-gui`）
   - `pnpm --dir ui test`
   - `pnpm --dir ui lint`
   - `pnpm --dir ui build`
   - `cargo clippy -p cruise-gui --all-targets`

4. **手動動作確認と PR 作成**
   - `pnpm --dir ui dev` / `cargo tauri dev` で GUI 起動
   - 既存セッションの DAG タブを開いて描画確認
   - 分岐 workflow でのエッジ描画確認
   - スクリーンショット撮影して PR body に添付
   - PR 作成前に `/review-all` を実行

## Next-Agent Starting Position
- ブランチ: `cruise/20260629065146988_fb6aecb7b2894d64b99b11d310e16eb5-GUI-DAG-PR-DAG`
- コミット `b27aa59` までの変更はコミット済み
- 次は `ui/src/App.tsx` の `ActiveTab` 拡張とタブボタン・パネルへの DAG 統合から着手
- 統合後、テストを追加・実行し、lint/build/clippy を通す
- 計画書 `/Users/takumi/.local/share/cruise/sessions/20260629065146988_fb6aecb7b2894d64b99b11d310e16eb5/plan.md` の未確定事項（step 単位表示で良いか、タブ配置位置）に留意
