#!/usr/bin/env bun
/**
 * jcode-migration-workflow.ts — seher 完全削除 / jcode + ClaudeSDK 移行ワークフロー
 *
 * JCODE.md のフェーズ P0..P5, P6a, P6b, P7 を、OMP SDK (`@oh-my-pi/pi-coding-agent`) を in-process で
 * 駆動して順に実行するシングルファイル Bun ワークフロー。
 * SDK は Bun 専用 (engines: {"bun": ">=1.3.14"}) のため Bun で実装する。
 * https://omp.sh/docs/sdk がワイヤ契約の正典。
 *
 * 構成は bun-in-rust (https://bun.com/blog/bun-in-rust) の手法に従う。フェーズごとに:
 *   1 implementer → 検証ゲート (失敗は同一セッションで fix turn, 最大3回)
 *   → 2 adversarial reviewers (別コンテキスト・read-only・並列)
 *   → 指摘があれば 1 fixer (新規セッション) → ゲート再検証
 *   → git commit
 * implementer は review せず、reviewer は実装しない。
 *
 * 429/5xx の同一モデルリトライは omp 本体が行う。別モデル自動フォールバックは
 * ~/.omp の settings で `retry.fallbackChains` を設定した場合のみ働く (既定は空 =
 * フォールバックなし)。本ワークフローは auto_retry_* / retry_fallback_* イベントを
 * 観測してログするだけ。`session.prompt()` は retry ライフサイクル完了まで解決
 * しないため、リトライ中のターンを完了扱いする事故は起きない。
 *
 * 使い方:
 *   bun add @oh-my-pi/pi-coding-agent   # 初回のみ (bun の auto-install でも可)
 *   bun scripts/jcode-migration-workflow.ts
 *   bun scripts/jcode-migration-workflow.ts --phase P2          # 1 フェーズのみ
 *   bun scripts/jcode-migration-workflow.ts --from P3           # P3 から再開
 *   bun scripts/jcode-migration-workflow.ts --model anthropic/claude-opus-4-6
 *   bun scripts/jcode-migration-workflow.ts --dry-run           # プロンプトとゲートの確認のみ (SDK 不要)
 *
 * 前提:
 *   - リポジトリルート (cruise) で実行。JCODE.md / PROHIBITED.md が存在すること。
 *   - git worktree が clean であること (フェーズ完了ごとに自動コミットするため、
 *     本計画ドキュメント群も先にコミットしておく)。
 *   - omp のプロバイダ認証が済んでいること (~/.omp/agent、`omp /login` 等)。
 *   - P0-P2 を含む実行では `../seher` (claude-agent-sdk のコピー元) がチェックアウト済みであること。
 *   - フェーズ失敗 (gate 3 回不通過等) で中断した場合、worktree には未コミットの変更が残る。
 *     内容を確認して手動で commit するか捨ててから (`git status` → commit / restore)、--from で再開する。
 */

import { tmpdir } from "node:os";

// ---------------------------------------------------------------------------
// SDK の構造的最小型 (--dry-run を SDK 未インストールでも動かすため動的 import する)
// ---------------------------------------------------------------------------

interface AgentSessionEvent {
  type: string;
  assistantMessageEvent?: { type?: string; delta?: string };
  // auto_retry_start (payload 形は sdk.md 未記載の推定 — 欠落時も落ちないよう ?? で表示)
  attempt?: number;
  maxAttempts?: number;
  delayMs?: number;
  errorMessage?: string;
  // retry_fallback_applied
  from?: string;
  to?: string;
  role?: string;
}

interface AgentSession {
  sessionFile?: string;
  subscribe(listener: (event: AgentSessionEvent) => void): () => void;
  prompt(text: string): Promise<void>;
  beginDispose(): void;
  dispose(): Promise<void>;
}

interface OmpSdk {
  createAgentSession(options?: Record<string, unknown>): Promise<{
    session: AgentSession;
    modelFallbackMessage?: string;
  }>;
  SessionManager: { create(cwd: string): unknown };
  AgentRegistry: new () => unknown;
}

async function loadSdk(): Promise<OmpSdk> {
  try {
    // 動的 import (静的 import の例外): --dry-run を SDK 未インストール環境でも
    // 動かすため、モジュール解決を実行パス選択後まで遅延させる必要がある。
    return (await import("@oh-my-pi/pi-coding-agent")) as unknown as OmpSdk;
  } catch (err) {
    throw new Error(
      `@oh-my-pi/pi-coding-agent の読み込みに失敗: ${err instanceof Error ? err.message : String(err)}\n` +
        "`bun add @oh-my-pi/pi-coding-agent` を実行するか、bun の auto-install が有効な環境で実行すること。",
    );
  }
}

// ---------------------------------------------------------------------------
// セッション: implementer / fixer は全ツール、reviewer は read-only
// ---------------------------------------------------------------------------

interface TurnStats {
  retries: number;
  fallbacks: string[];
}

interface SessionOptions {
  model: string;
  quiet: boolean;
  /** reviewer 用: read/grep/glob のみに制限 (restrictToolNames は MCP/拡張/LSP も無効化する) */
  readOnly?: boolean;
}

class PhaseSession {
  #session: AgentSession;
  #unsubscribe: () => void = () => {};
  #quiet: boolean;
  #turnText = "";
  /** セッション生涯の累計 (fix turn 中のリトライも含む) */
  readonly stats: TurnStats = { retries: 0, fallbacks: [] };

  private constructor(session: AgentSession, quiet: boolean) {
    this.#session = session;
    this.#quiet = quiet;
  }

  static async open(sdk: OmpSdk, cwd: string, opts: SessionOptions): Promise<PhaseSession> {
    const { session, modelFallbackMessage } = await sdk.createAgentSession({
      cwd,
      // ファイル永続: 各セッションの transcript を後から監査できるようにする
      sessionManager: sdk.SessionManager.create(cwd),
      // reviewer 2 並列を含むトップレベルセッションが "Main" identity で
      // 衝突しないよう私有レジストリを渡す (sdk.md の指示)
      agentRegistry: new sdk.AgentRegistry(),
      ...(opts.model ? { modelPattern: opts.model } : {}),
      ...(opts.readOnly ? { toolNames: ["read", "grep", "glob"], restrictToolNames: true } : {}),
    });
    if (modelFallbackMessage) console.error(`[model] ${modelFallbackMessage}`);
    if (session.sessionFile) console.error(`[session] ${session.sessionFile}`);

    const holder = new PhaseSession(session, opts.quiet);
    holder.#unsubscribe = session.subscribe((event) => holder.#onEvent(event));
    return holder;
  }

  #onEvent(event: AgentSessionEvent): void {
    switch (event.type) {
      case "message_update": {
        const msg = event.assistantMessageEvent;
        if (msg?.type === "text_delta" && msg.delta) {
          this.#turnText += msg.delta;
          if (!this.#quiet) process.stdout.write(msg.delta);
        }
        break;
      }
      case "auto_retry_start":
        this.stats.retries++;
        console.error(
          `\n[retry] attempt ${event.attempt ?? "?"}/${event.maxAttempts ?? "?"} in ${event.delayMs ?? "?"}ms: ${event.errorMessage ?? "(no message)"}`,
        );
        break;
      case "retry_fallback_applied":
        this.stats.fallbacks.push(`${event.from ?? "?"} -> ${event.to ?? "?"}`);
        console.error(`\n[fallback] ${event.from ?? "?"} -> ${event.to ?? "?"} (role: ${event.role ?? "?"})`);
        break;
      case "retry_fallback_succeeded":
        console.error(`\n[fallback] succeeded`);
        break;
      case "auto_retry_end":
        console.error(`\n[retry] end`);
        break;
    }
  }

  /** prompt() は retry ライフサイクル完了まで解決する — これが完了シグナル。戻り値はターンの assistant テキスト。 */
  async prompt(text: string): Promise<string> {
    this.#turnText = "";
    await this.#session.prompt(text);
    return this.#turnText;
  }

  async close(): Promise<void> {
    this.#session.beginDispose(); // 最初の await より前: 遅延ワークの流入を止める (sdk.md)
    this.#unsubscribe();
    await this.#session.dispose();
  }
}

// ---------------------------------------------------------------------------
// サブプロセス実行と検証ゲート
// ---------------------------------------------------------------------------

interface RunResult {
  code: number;
  stdout: string;
  stderr: string;
}

async function run(cmd: string[], cwd: string): Promise<RunResult> {
  const proc = Bun.spawn(cmd, { cwd, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  return { code, stdout, stderr };
}

interface Gate {
  label: string;
  cmd: string[];
  /**
   * 期待する終了コード (既定 0)。残留 grep は 1 (= 一致 0 件) を指定する。
   * grep の exit 2 (パス欠落等のエラー) を成功扱いしないため、「非ゼロなら成功」ではなく完全一致で判定する。
   */
  expectExit?: number;
}

interface GateFailure {
  label: string;
  output: string;
}

function describeGate(gate: Gate): string {
  const expected = gate.expectExit ?? 0;
  const cond =
    expected === 0 ? "exit 0" : gate.cmd[0] === "grep" ? `一致 0 件 (exit ${expected})` : `exit ${expected}`;
  return `\`${gate.cmd.join(" ")}\` が ${cond} で成功`;
}

async function runGate(gate: Gate, cwd: string): Promise<GateFailure | null> {
  console.error(`\n[gate] ${gate.label}: ${gate.cmd.join(" ")}`);
  let result: RunResult;
  try {
    result = await run(gate.cmd, cwd);
  } catch (err) {
    // spawn 失敗 (ENOENT 等) はワークフロー全体を落とさずゲート失敗として扱う
    console.error(`[gate] ${gate.label}: SPAWN FAILED`);
    return { label: gate.label, output: `failed to spawn: ${err instanceof Error ? err.message : String(err)}` };
  }
  const expected = gate.expectExit ?? 0;
  if (result.code === expected) {
    console.error(`[gate] ${gate.label}: OK`);
    return null;
  }
  console.error(`[gate] ${gate.label}: FAILED (exit ${result.code}, expected ${expected})`);
  return { label: gate.label, output: (result.stderr + "\n" + result.stdout).slice(-8000) }; // 末尾 8KB のみ引き渡す
}

// ---------------------------------------------------------------------------
// フェーズ定義 (JCODE.md §5 に対応)
// ---------------------------------------------------------------------------

interface Phase {
  id: string;
  title: string;
  task: string;
  gates: Gate[];
}

const PREAMBLE = `まず JCODE.md と PROHIBITED.md を読み、両方の内容に厳密に従うこと。
PROHIBITED.md の禁止事項 (設定ファイル形式の変更禁止、呼び出し面の破壊禁止、
jcode フォーク禁止、seher の alias/shim 禁止、スコープ外リファクタリング禁止 等) に
違反する変更は一切行わない。作業は現在のフェーズの範囲のみ。
git は読み取り (status / diff / log / show) のみ使用可。commit / stash / reset /
checkout / restore は実行しない — コミットはワークフロー側が行う。
ワークアラウンドの正当化に段落コメントが必要なら、そのコードが間違っている。コードを直すこと。
完了したら変更内容を要約すること。
`;

const cargo = (label: string, ...args: string[]): Gate => ({ label, cmd: ["cargo", ...args] });

const PHASES: Phase[] = [
  {
    id: "P0",
    title: "前提整備 — crate の取り込み",
    task: `JCODE.md フェーズ P0 を実施:
1. Cargo.toml の [dependencies] に \`seher-claude-agent-sdk = "=0.0.59"\` を追加する。
2. crates.io の published .crate (0.0.59) を取得して vendor/claude-agent-sdk/ へ展開し (例: https://static.crates.io/crates/seher-claude-agent-sdk/seher-claude-agent-sdk-0.0.59.crate をリポジトリ外の一時ディレクトリで tar xz)、[patch.crates-io] に seher-claude-agent-sdk = { path = "vendor/claude-agent-sdk" } を追加する (vendor/asupersync と同一パターン。workspace member にはしない — crates.io publish を壊さない、JCODE.md ゴール 7)。
3. ソース無改変。出自 (crates.io 0.0.59、checksum) を vendor/claude-agent-sdk/ 配下の README に記録し、.crate に LICENSE ファイルが無ければ ../seher/LICENSE (Apache-2.0) をコピーして同梱する (PROHIBITED §5)。
4. seher-sdk 依存はこのフェーズでは削除しない。jcode 系 crate の vendor はこのフェーズでは行わない (P3 スパイクの結果次第)。`,
    gates: [
      cargo("build", "build"),
      {
        // seher-sdk が transitively に同 crate へ依存済みのため、存在確認では空虚。
        // patch 適用時のみ cargo tree のパッケージ行に vendor パスが付くことを検査する。
        label: "patch applied",
        cmd: ["bash", "-c", "cargo tree -i seher-claude-agent-sdk | grep -q 'vendor/claude-agent-sdk'"],
      },
    ],
  },
  {
    id: "P1",
    title: "ローカル型の導入",
    task: `JCODE.md フェーズ P1 を実施:
StreamChunk{Delta,Done,Session,Limit,Error} / ChunkOutcome 系 / EffortLevel / split_thinking_suffix / CruiseTool (旧 SeherTool と同形: name, description, parameters, handler = Arc<dyn Fn(serde_json::Value) -> Result<String, String> + Send + Sync>) を cruise ローカルモジュールとして定義し、src/sdk_tools.rs を CruiseTool ベースに切り替える。6 ツールの handler 本体・スキーマ・エラー文言は変更しない。executor.rs はこの段階では seher 型と共存させる (必要なら From 変換)。
注意: seher の split_thinking_suffix は内部で pi crate の ThinkingLevel::from_str に委譲している。cruise ローカル版は effort 5 段 (low|medium|high|xhigh|max) をローカル定義した上で同等の挙動を再実装すること (pi crate への依存を持ち込まない)。`,
    gates: [cargo("build", "build"), cargo("test sdk_tools", "test", "sdk_tools")],
  },
  {
    id: "P2",
    title: "Claude backend",
    task: `JCODE.md フェーズ P2 (§3.3) を実施:
src/backend/claude.rs を新規実装する。../seher/crates/seher-sdk/src/claude_agent/mod.rs を参照実装として、ClaudeAgentOptions 構築 / Message ストリーム → StreamChunk 変換 / stderr 64 行リング + rate-limit 判定 / 「tools あり時は SubprocessCliTransport::streaming + user_message_frame + end_input」ワークアラウンド (one_shot だと SDK MCP server の initialize が失敗する) を cruise 側 glue として書く。permission_mode 既定は BypassPermissions。Executor に Claude variant を追加し SUPPORTED_SDKS に "claude" を追加する (seher/pi はまだ残す)。session resume は ClaudeAgentOptions::resume 経由。
claude CLI が実行可能・認証済みの環境であれば、JCODE.md P2 受け入れ基準の sdk: claude 最小スモーク (plan で対話ツールが呼ばれる + run step 1 つ) を一時ディレクトリで自分でも実行して確認する。実行できない場合は理由を報告する (実行していない検証を成功と報告しない)。`,
    gates: [cargo("build", "build"), cargo("test executor", "test", "executor")],
  },
  {
    id: "P3",
    title: "ToolBridge + Jcode backend + cruise login",
    task: `JCODE.md フェーズ P3 (§3.4–§3.6) を実施:
0. 冒頭スパイク: §3.4 案 B' (JCODE_HOME 分離の jcode run --ndjson subprocess) の検証項目 (1)-(8) を実 jcode バイナリで確認し、結果を JCODE.md §3.4 に追記する。成立 → 案 B' (vendor なし) で実装。不成立 → 案 A: jcode リポジトリ (a5f17d2) から jcode-sdk / jcode-harness-api / jcode-transport を vendor/ へコピー (MIT・SHA 記録) し、自名義 publish + [patch.crates-io] 前提で組み込む (JCODE.md ゴール 7 維持)。
1. 隠しサブコマンド \`cruise mcp-bridge\`: stdio 上の MCP server (JSON-RPC 2.0: initialize / tools/list / tools/call) として動き、全ツール呼び出しを Unix socket 経由で親 cruise プロセスへ転送する。socket パスは env CRUISE_TOOL_SOCKET から取得 (--socket <path> は override)。
2. 親側 ToolBridge: prompt 実行ごとに Unix socket を listen し、Vec<CruiseTool> の handler を同期実行して結果を返す。
3. src/backend/jcode.rs: 採用案でイベント → StreamChunk 写像 / resume / モデル・effort 指定 (§3.4)。MCP 登録は §3.6 のとおり並行安全に: <jcode_home>/mcp.json は固定内容 {"mcpServers":{"cruise":{"command":<current_exe>,"args":["mcp-bridge"]}}} とし、socket パスは jcode run の env CRUISE_TOOL_SOCKET で継承させる (スパイク (7))。mcp.json の書き直しは current_exe 変化時のみ tmp+rename + flock。認証分離は JCODE_HOME=<cruise 専用 home> (案 A: inherit_logins=false 併用)、JCODE_NO_TELEMETRY=1、自動アップデート無効。Executor に Jcode variant を追加。
   注意 (JCODE.md §2.5): jcode の MCP 設定は後勝ちマージで project-local (.jcode/mcp.json / .mcp.json / .claude/mcp.json) が $JCODE_HOME/mcp.json より優先される。対象 repo に MCP 設定がある場合の挙動を実測し、cruise の server 名衝突・意図しない server の load を検知して警告すること。
4. 認証分離 (JCODE.md §3.5): 新設サブコマンド \`cruise login\` を実装する — 引数なし/provider 指定は JCODE_HOME=<cruise home> で \`jcode login\` を exec。--api-key / --status の実現手段はスパイク (3)(4) の結果に従う (jcode の保存形式・実装をそのまま使う薄いラッパに徹する)。未認証で sdk: jcode を実行した場合は cruise login を案内する明確なエラーを出す。
ユーザの ~/.jcode (認証含む) と対象リポジトリには読み書きとも一切触れないこと (PROHIBITED §6)。
単体テストは jcode バイナリの存在に依存させない (mock / socket レベルで検証。既存テストの慣例どおりユビキタスなコマンドのみ使用) — 検証ゲートの cargo test は jcode 未インストール環境でも通ること。`,
    gates: [cargo("build", "build"), cargo("test", "test")],
  },
  {
    id: "P4",
    title: "OMP ライク fallback エンジン",
    task: `JCODE.md フェーズ P4 (§3.7) を実施:
src/retry.rs を新規実装: classify_retryable (429/500/502/503/504/overloaded/rate limit/network 系。既存 step/command.rs の is_rate_limited と seher の is_claude_rate_limit_message の判定を統合)、バックオフ min(base_delay_ms * 2^(attempt-1), 8000ms) * jitter(0.75..1.00)、Retry-After ヒント優先 (60s クランプ)、fallback_chains の特異性順選択 (完全一致 provider/model → provider/* → default)、モデル切替時は遅延 0 + fresh リトライ予算、失敗モデルの in-memory cooldown。
cruise.yaml に optional な retry: ブロック (base_delay_ms / max_delay_ms / model_fallback / fallback_chains) を追加 (config parse + validate + cruise-schema.json)。retry: 未設定時の挙動は従来と完全一致であること (PROHIBITED §7)。リトライ回数は既存 PromptRun::max_retries を流用し新設しない。run_jcode / run_claude のリトライループに組み込む。再試行は常に fresh session、可視 Delta 出力後のターンはリトライしない。単体テストで「429 → chain 次モデルへ遅延 0 切替」「chain 枯渇 → 失敗」「retry 未設定 → 従来挙動」を検証する。`,
    gates: [cargo("build", "build"), cargo("test retry", "test", "retry")],
  },
  {
    id: "P5",
    title: "seher 完全削除",
    task: `JCODE.md フェーズ P5 を実施:
Cargo.toml から seher-sdk を削除。run_sdk / run_pi_direct / build_pi_options / parse_pi_model_ref / resolve_provider / poll_for_agent スレッド / spawn_agent_stream の 6 分岐 / merge_helper_env / finish_sdk_session / mode_key_for_step / mode_key_for_plan / require_tools による provider 絞り込みを削除。planning.rs の sdk_plan_tools_enabled は sdk 値非依存 (sdk.is_some() && interactive_planning) かつ src-tauri が使用するため削除しない (doc コメントの seher/pi 言及のみ更新)。planning.rs:466 の pi_session_path transcript 診断は採用案の手段 (案 B': $JCODE_HOME 配下セッションファイル読み or 診断なし、案 A: peek_session) に置換、claude backend では None。SUPPORTED_SDKS = ["jcode", "claude"]。validate_sdk: sdk/command 両未指定を error から jcode 既定へ変更 (additive — PROHIBITED §1 許可差分 3)、Executor::new(None, 空 command) は Jcode variant を返す。builtin/cruise.yaml から sdk: / model: / plan_model: 行を削除し、builtin 既定を assert するテスト (config.rs:1541-1543) を追随。cruise-schema.json の sdk enum 更新。旧値 sdk: seher / sdk: pi は移行案内つき validation error で拒否 (alias 禁止。エラーメッセージが旧値名に言及するのは当然許容される)。"seher"/"pi" 文字列を使うテストは検証意図を保って新値へ更新し、sdk/command 両未指定 → jcode 既定のテストを追加。`,
    gates: [
      cargo("build workspace", "build", "--workspace"),
      cargo("test", "test"),
      {
        // 依存としての seher 残留のみ検査する。validation error の文言や
        // テスト内の "seher" 文字列 (PROHIBITED §3 が要求する拒否パス) は許容。
        label: "no seher dependency",
        cmd: ["grep", "-rEn", "seher(::|-sdk|_sdk)", "src/", "Cargo.toml"],
        expectExit: 1,
      },
      {
        label: "no seher/pi in config surface",
        cmd: ["grep", "-rniE", "seher|\"pi\"|sdk: *pi", "builtin/", "cruise-schema.json"],
        expectExit: 1,
      },
    ],
  },
  {
    id: "P6a",
    title: "GitHub Actions 移行",
    task: `JCODE.md フェーズ P6a を実施:
action は現在「常に sdk: pi・in-process 実行・外部バイナリ不要」前提 (action.yml、resolve-config.sh ヘッダ)。「sdk 指定なし = 既定 jcode」へ移行する:
1. action/scripts/resolve-config.sh の生成 config (default / exec 用) から sdk 強制を除去する (sdk: 行を書かない)。
2. action.yml に jcode バイナリの install ステップを追加する (cruise 本体と同様にバージョン pin 可能な入力を用意)。
3. anthropic_api_key / openai_api_key / provider_api_keys 入力を action 管理の JCODE_HOME 配下 <provider>.env へ投入し、providers 入力を同 config.toml の [providers.<name>] プロファイル生成に写像する (PI_MODELS_JSON → models.json パイプラインの全面置換)。
4. model / plan_model 入力 (CRUISE_MODEL / CRUISE_PLAN_MODEL) が provider/model[:effort] 参照としてそのまま機能することを確認する。
5. scripts/test_action_*.sh を検証意図を保って書き換える (models.json 生成の assert → config.toml / <provider>.env 生成の assert。テストの削除・弱体化は禁止)。
6. action.yml の最低 cruise version 要件を更新する (現行の「Requires cruise v0.1.68 or later」記述と gate step を v0.2.0 以降必須に。旧 binary + 新 action の組合せを明確なエラーで拒否)。`,
    gates: [
      { label: "action config tests", cmd: ["bash", "scripts/test_action_config_install.sh"] },
      { label: "action provider tests", cmd: ["bash", "scripts/test_action_provider_config.sh"] },
      {
        label: "no seher/pi in action",
        cmd: ["grep", "-rniE", "seher|sdk: *pi", "action.yml", "action/scripts/"],
        expectExit: 1,
      },
    ],
  },
  {
    id: "P6b",
    title: "ドキュメント更新",
    task: `JCODE.md フェーズ P6b を実施:
README.md の seher/pi 言及全箇所 (backend 節のほか、grep で列挙して漏れなく: sdk: claude-terminal 言及、skip_step のツール対象記述、GitHub Actions 例なども含む)、docs/github-actions.md、skills/cruise-config/references/sdk.md (および SKILL.md / examples)、skills/cruise-cli、skills/cruise-plan、examples/*.yaml、prompts/*-sdk.md (ツール名参照があれば) の seher/pi 記述を jcode/claude へ更新。sdk 省略が既定 (jcode) である旨を README / skills に反映する。記述は実装した挙動 (JCODE.md §3, §4) と正確に一致させること。ドキュメントに存在しない機能を書かない。移行案内は validation error が担うため、ドキュメント本文に seher/pi の記述を残さない。`,
    gates: [
      {
        label: "no seher/pi in docs",
        cmd: ["grep", "-rniE", "seher|sdk: *pi", "README.md", "docs/", "skills/", "examples/", "prompts/"],
        expectExit: 1,
      },
    ],
  },
  {
    id: "P7",
    title: "スモーク検証",
    task: `JCODE.md フェーズ P7 を実施:
1. sdk: claude の最小 workflow yaml を一時ディレクトリに作り、cruise plan (非対話: interactive_planning の非 TTY 経路で submit_plan が呼ばれ plan.md が生成されること) と run step 1 つをスモーク実行する。PR 説明生成 (submit_pr_metadata) と title 生成 (generate_title) の経路も一度ずつ発火させる。
2. jcode がインストール済みなら sdk: jcode でも同じスモークを実行し、mcp__cruise__submit_plan の発火と resume (fix-plan ターン) を確認する。jcode 未インストールならその旨を報告し claude 側の結果のみ報告する。
3. fallback: 実プロバイダで 429 を誘発できない場合は classify_retryable のフィクスチャ単体テストで代替し、その旨を記録する。
4. cargo test --workspace を全実行する (src-tauri のビルドを含む)。
5. 実行できた検証・できなかった検証を明確に区別して最終報告すること。実行していない検証を成功と報告してはならない。`,
    gates: [cargo("test workspace", "test", "--workspace")],
  },
];

// ---------------------------------------------------------------------------
// adversarial review (bun-in-rust 方式: 2 reviewers 別コンテキスト + 1 fixer)
// ---------------------------------------------------------------------------

const REVIEWER_COUNT = 2;
const MAX_STAT_LINES = 200;

/** vendor コピー (patch 用 crate 等) は diff 本文から除外する (--stat には出る)。reviewer は必要なら実ファイルを読む。 */
const DIFF_EXCLUDES = [":(exclude)vendor/*"];

interface ReviewInput {
  stat: string;
  diffPath: string | null;
}

/** フェーズの変更を stage して diff を採取する。変更が無ければ null。 */
async function collectDiff(cwd: string, phaseId: string): Promise<ReviewInput | null> {
  const add = await run(["git", "add", "-A"], cwd);
  if (add.code !== 0) throw new Error(`git add failed: ${add.stderr}`);
  const stat = await run(["git", "diff", "--cached", "--stat"], cwd);
  if (stat.code !== 0) throw new Error(`git diff --stat failed: ${stat.stderr}`);
  if (!stat.stdout.trim()) return null;

  const statLines = stat.stdout.trimEnd().split("\n");
  const statText =
    statLines.length > MAX_STAT_LINES
      ? `${statLines.slice(0, MAX_STAT_LINES).join("\n")}\n... (${statLines.length - MAX_STAT_LINES} 行省略)`
      : stat.stdout.trimEnd();

  const diff = await run(["git", "diff", "--cached", "--", ".", ...DIFF_EXCLUDES], cwd);
  if (diff.code !== 0) throw new Error(`git diff failed: ${diff.stderr}`);
  let diffPath: string | null = null;
  if (diff.stdout.trim()) {
    diffPath = `${tmpdir()}/cruise-review-${phaseId}-${Date.now()}.diff`;
    await Bun.write(diffPath, diff.stdout);
  }
  return { stat: statText, diffPath };
}

function reviewerPrompt(phase: Phase, input: ReviewInput): string {
  return `あなたは adversarial code reviewer。実装・ファイル編集は一切しない。
唯一の仕事は、この変更がバグを生む・動かない・仕様に違反する理由を徹底的に挙げること。

まず JCODE.md と PROHIBITED.md を読むこと。レビュー対象はフェーズ ${phase.id} (${phase.title}) の実装:

## フェーズのタスク定義
${phase.task}

## 変更ファイル一覧 (git diff --stat)
${input.stat}

${
  input.diffPath
    ? `完全な diff (vendor コピー除く) は ${input.diffPath} を読むこと。`
    : "diff 本文は vendor コピーのみ (上の --stat 参照)。実ファイルを直接読んで検査すること。"
}
必要に応じてリポジトリ内の実ファイルも読み、diff の前後の文脈を確認すること。

チェック観点:
- 正しさ: 挙動変化、エラーパス、リソースリーク、部分適用 (呼び出し元の直し漏れ)
- JCODE.md のフェーズ定義・受け入れ基準との乖離
- PROHIBITED.md 違反: スタブ / todo!() / テスト削除・弱体化 / スコープ外変更 / alias・shim / ユーザ環境汚染
- ワークアラウンドの正当化に段落コメントを要するコード (= コード自体が誤り、修正対象として指摘)

出力: 指摘ごとに「ファイル:行 — 問題 — 具体的な失敗シナリオ」を 1 項目ずつ。
最終行は必ず \`VERDICT: OK\` (指摘なし) か \`VERDICT: ISSUES\` の単独行にすること。`;
}

function verdictOf(text: string): "OK" | "ISSUES" {
  const matches = text.match(/^VERDICT:\s*(OK|ISSUES)\s*$/gm);
  const last = matches?.at(-1);
  if (!last) return "ISSUES"; // verdict 欠落は保守的に指摘あり扱い
  return last.includes("OK") ? "OK" : "ISSUES";
}

interface WorkflowOptions {
  model: string;
  quiet: boolean;
  noReview: boolean;
}

async function runReviewer(
  sdk: OmpSdk,
  cwd: string,
  opts: WorkflowOptions,
  phase: Phase,
  input: ReviewInput,
  n: number,
): Promise<string> {
  // reviewer は常に quiet (2 並列のストリームが stdout で交錯するため) + read-only
  const session = await PhaseSession.open(sdk, cwd, { model: opts.model, quiet: true, readOnly: true });
  try {
    console.error(`[review] reviewer ${n}/${REVIEWER_COUNT} started`);
    const text = await session.prompt(reviewerPrompt(phase, input));
    console.error(`[review] reviewer ${n}/${REVIEWER_COUNT}: VERDICT ${verdictOf(text)}`);
    return text;
  } finally {
    await session.close();
  }
}

function fixerPrompt(phase: Phase, reviews: string[]): string {
  const sections = reviews.map((r, i) => `## Reviewer ${i + 1}\n${r}`).join("\n\n");
  return `${PREAMBLE}
# フェーズ ${phase.id} (${phase.title}) — レビュー指摘の修正

${REVIEWER_COUNT} 名の adversarial reviewer がこのフェーズの変更 (git diff HEAD / staged) に以下の指摘を出した。
妥当な指摘をすべて修正すること。誤検出と判断した指摘は修正せず、判断理由を 1 行で述べること (議論はしない)。

${sections}`;
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

const MAX_FIX_ATTEMPTS = 3;

const USAGE = `usage: bun scripts/jcode-migration-workflow.ts [options]
  --phase <id>       指定フェーズのみ実行 (P0..P5, P6a, P6b, P7)
  --from <id>        指定フェーズから最後まで実行
  --model <pattern>  omp のモデルパターン (例: anthropic/claude-opus-4-6)
  --dry-run          プロンプトとゲートの確認のみ (SDK 不要)
  --quiet            assistant テキストのストリーム表示を抑制
  --no-review        adversarial review をスキップ (デバッグ用)
  --help             このヘルプ`;

function parseArgs(argv: string[]) {
  const opts = { phase: "", from: "", model: "", dryRun: false, quiet: false, noReview: false, help: false };
  const need = (flag: string, value: string | undefined): string => {
    if (!value || value.startsWith("--")) throw new Error(`${flag} requires a value\n${USAGE}`);
    return value;
  };
  for (let i = 0; i < argv.length; i++) {
    switch (argv[i]) {
      case "--phase": opts.phase = need("--phase", argv[++i]); break;
      case "--from": opts.from = need("--from", argv[++i]); break;
      case "--model": opts.model = need("--model", argv[++i]); break;
      case "--dry-run": opts.dryRun = true; break;
      case "--quiet": opts.quiet = true; break;
      case "--no-review": opts.noReview = true; break;
      case "--help":
      case "-h": opts.help = true; break;
      default: throw new Error(`unknown argument: ${argv[i]}\n${USAGE}`);
    }
  }
  if (opts.phase && opts.from) throw new Error(`--phase と --from は併用できない\n${USAGE}`);
  return opts;
}

async function preflight(cwd: string, phases: Phase[]): Promise<void> {
  for (const f of ["JCODE.md", "PROHIBITED.md", "Cargo.toml"]) {
    if (!(await Bun.file(`${cwd}/${f}`).exists())) {
      throw new Error(`preflight: ${f} not found in ${cwd} — run from the cruise repo root`);
    }
  }
  for (const bin of ["git", "cargo", "grep", "bash"]) {
    if (!Bun.which(bin)) throw new Error(`preflight: \`${bin}\` not found in PATH`);
  }
  // ../seher は P0 (LICENSE コピー元) と P1/P2 (参照実装) でのみ必要
  const needsSeher = phases.some((p) => p.id === "P0" || p.id === "P1" || p.id === "P2");
  if (needsSeher && !(await Bun.file(`${cwd}/../seher/crates/claude-agent-sdk/Cargo.toml`).exists())) {
    throw new Error("preflight: ../seher/crates/claude-agent-sdk not found — checkout seher next to cruise");
  }
  // P3 は冒頭スパイク (JCODE.md §3.4 案 B' 検証) に実 jcode バイナリが必須
  if (phases.some((p) => p.id === "P3") && !Bun.which("jcode")) {
    throw new Error("preflight: `jcode` not found in PATH — P3 のスパイク検証に必要 (https://github.com/1jehuang/jcode)");
  }
  // フェーズごとに自動コミットするため、開始時点の worktree は clean であること
  const status = await run(["git", "status", "--porcelain"], cwd);
  if (status.code !== 0) throw new Error(`preflight: git status failed: ${status.stderr}`);
  if (status.stdout.trim()) {
    throw new Error(
      "preflight: git worktree is not clean — フェーズ完了ごとに自動コミットするため、既存の変更 (本計画ドキュメント群を含む) を先にコミットすること",
    );
  }
}

/** ゲートを全通過するまで fix turn を回す。session は implementer または fixer。 */
async function passGates(phase: Phase, cwd: string, session: PhaseSession): Promise<void> {
  let attempt = 0;
  while (true) {
    const failures: GateFailure[] = [];
    for (const gate of phase.gates) {
      const failure = await runGate(gate, cwd);
      if (failure) failures.push(failure);
    }
    if (failures.length === 0) return;
    if (++attempt > MAX_FIX_ATTEMPTS) {
      throw new Error(
        `${phase.id} gates still failing after ${MAX_FIX_ATTEMPTS} fix attempts: ${failures.map((f) => f.label).join(", ")}`,
      );
    }
    console.error(`\n[fix] ${phase.id} gate failures (attempt ${attempt}/${MAX_FIX_ATTEMPTS})`);
    await session.prompt(
      `検証ゲートが失敗した。PROHIBITED.md に違反しない範囲で修正すること (テストの削除・無効化・アサーション弱体化は禁止)。\n\n` +
        failures.map((f) => `## ${f.label}\n\`\`\`\n${f.output}\n\`\`\``).join("\n\n"),
    );
  }
}

async function commitPhase(phase: Phase, cwd: string): Promise<void> {
  const status = await run(["git", "status", "--porcelain"], cwd);
  if (!status.stdout.trim()) {
    console.error(`[commit] ${phase.id}: no changes to commit`);
    return;
  }
  const add = await run(["git", "add", "-A"], cwd);
  if (add.code !== 0) throw new Error(`git add failed: ${add.stderr}`);
  const commit = await run(["git", "commit", "-m", `jcode-migration ${phase.id}: ${phase.title}`], cwd);
  if (commit.code !== 0) throw new Error(`git commit failed: ${commit.stderr}\n${commit.stdout}`);
  console.error(`[commit] ${phase.id} committed`);
}

function taskPrompt(phase: Phase): string {
  const gateList = phase.gates.map((g) => `- ${describeGate(g)}`).join("\n");
  return `${PREAMBLE}
# フェーズ ${phase.id}: ${phase.title}

${phase.task}

完了後にワークフロー側で次の検証ゲートを実行する:
${gateList}
すべてのゲートが成功する状態で作業を終えること。`;
}

async function runPhase(sdk: OmpSdk, cwd: string, opts: WorkflowOptions, phase: Phase): Promise<void> {
  console.error(`\n\n========== ${phase.id}: ${phase.title} ==========\n`);

  // implementer: フェーズごとに独立セッション (前フェーズの巨大な履歴を持ち越さない)
  const implementer = await PhaseSession.open(sdk, cwd, { model: opts.model, quiet: opts.quiet });
  try {
    await implementer.prompt(taskPrompt(phase));
    await passGates(phase, cwd, implementer);
    if (implementer.stats.fallbacks.length > 0) {
      console.error(`[info] model fallbacks during ${phase.id}: ${implementer.stats.fallbacks.join(", ")}`);
    }
  } finally {
    await implementer.close();
  }

  // adversarial review: implementer とは別コンテキストの read-only reviewer を並列で走らせ、
  // 指摘があれば別の fixer セッションが適用してゲートを再検証する
  if (!opts.noReview) {
    const input = await collectDiff(cwd, phase.id);
    if (input === null) {
      console.error(`[review] ${phase.id}: no changes, skipping review`);
    } else {
      const reviews = await Promise.all(
        Array.from({ length: REVIEWER_COUNT }, (_, i) => runReviewer(sdk, cwd, opts, phase, input, i + 1)),
      );
      if (reviews.some((r) => verdictOf(r) === "ISSUES")) {
        console.error(`\n[fix] ${phase.id}: applying review findings`);
        const fixer = await PhaseSession.open(sdk, cwd, { model: opts.model, quiet: opts.quiet });
        try {
          await fixer.prompt(fixerPrompt(phase, reviews));
          await passGates(phase, cwd, fixer);
        } finally {
          await fixer.close();
        }
      } else {
        console.error(`[review] ${phase.id}: both reviewers OK`);
      }
    }
  }

  await commitPhase(phase, cwd);
  console.error(`\n[done] ${phase.id} complete`);
}

async function main(): Promise<void> {
  const opts = parseArgs(Bun.argv.slice(2));
  if (opts.help) {
    console.log(USAGE);
    return;
  }
  const cwd = process.cwd();

  let phases = PHASES;
  if (opts.phase) {
    phases = PHASES.filter((p) => p.id === opts.phase);
    if (phases.length === 0) throw new Error(`unknown phase: ${opts.phase}`);
  } else if (opts.from) {
    const idx = PHASES.findIndex((p) => p.id === opts.from);
    if (idx < 0) throw new Error(`unknown phase: ${opts.from}`);
    phases = PHASES.slice(idx);
  }

  if (opts.dryRun) {
    for (const p of phases) {
      const gates = p.gates.map((g) => describeGate(g)).join("\n  ");
      console.log(`\n=== ${p.id}: ${p.title} ===\n${p.task}\ngates:\n  ${gates}`);
    }
    console.log(`\nreview: ${opts.noReview ? "skipped (--no-review)" : `${REVIEWER_COUNT} adversarial reviewers + fixer per phase`}`);
    return;
  }

  await preflight(cwd, phases);
  const sdk = await loadSdk();

  for (const phase of phases) {
    await runPhase(sdk, cwd, opts, phase);
  }
  console.error("\n[workflow] all phases complete");
}

if (import.meta.main) {
  main().catch((err) => {
    console.error(`\n[workflow] FAILED: ${err instanceof Error ? err.message : String(err)}`);
    process.exit(1);
  });
}
