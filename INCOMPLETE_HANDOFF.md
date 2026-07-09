# Handoff: Planning context overflow - Partial Implementation

## What's Done

1. **`src/planning.rs`** - Fully implemented:
   - `extract_terminal_error_from_transcript()` - Pure function to parse JSONL transcripts for terminal errors
   - `resolve_generated_plan_content()` - New resolver that falls back to transcript errors when plan/stdout/stderr are empty
   - Full unit test coverage for both functions (lines 659-825)

2. **`src/plan_cmd.rs`** - Updated:
   - `generate_plan_markdown()` now uses `resolve_generated_plan_content` with transcript
   - Added `read_sdk_transcript()` helper function using `seher::sdk::pi_session_path`

3. **`src-tauri/src/commands.rs`** - Partially updated:
   - `create_session` (new plan) - Updated to use new resolver with transcript
   - `regenerate_plan` - NOT YET UPDATED (still uses `cruise::metadata::resolve_plan_content`)
   - `fix_plan` - NOT YET UPDATED (still uses `cruise::metadata::resolve_plan_content`)

## What Remains

1. **Update `src-tauri/src/commands.rs` regenerate_plan** (~line 1789):
   - Change `&mut None` to `let mut resume = None;` and pass `&mut resume`
   - Add transcript reading logic after `run_plan_prompt_template`
   - Use `cruise::planning::resolve_generated_plan_content` instead of `cruise::metadata::resolve_plan_content`

2. **Update `src-tauri/src/commands.rs` fix_plan** (~line 1957):
   - Same pattern as regenerate_plan

3. **Add `read_sdk_transcript` helper to `src-tauri/src/commands.rs`**:
   ```rust
   fn read_sdk_transcript(working_dir: Option<&std::path::Path>, session_id: &str) -> Option<String> {
       let transcript_path = seher::sdk::pi_session_path(working_dir, session_id);
       std::fs::read_to_string(&transcript_path).ok()
   }
   ```

4. **Update `prompts/plan-sdk.md`** with context budget discipline:
   - Add constraints before the `submit_plan` instruction
   - Content: use narrow `rg` queries, avoid huge alternation greps, read only needed line ranges, stop when evidence is gathered, keep tool output limits small

5. **Run `cargo fmt`**

6. **Run `cargo test`** to verify all tests pass

## Next-Agent Starting Position

The commit `96d73ff` contains the partial implementation. The remaining work is mechanical - follow the same pattern used in `create_session` for the two other Tauri command functions.

Key files:
- `/Users/takumi/.local/share/cruise/worktrees/20260709034455541_c6abe48b368a4f8983194c444081e083/src-tauri/src/commands.rs`
- `/Users/takumi/.local/share/cruise/worktrees/20260709034455541_c6abe48b368a4f8983194c444081e083/prompts/plan-sdk.md`
