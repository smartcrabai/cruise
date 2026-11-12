# Plan revision contract

`plan_revision.qnt` and `plan_revision.als` check the modeled planning-action offers for idle sessions. They do not prove client parity, phase preservation, or prompt delivery against production; `tests/plan_revision_bdd.rs` checks those behaviors against the production clients and application, including negative controls. Run the formal and production-connected checks with `bash scripts/check_plan_revision_specs.sh`.

## Modeled action offers

The models cover only the review-action subset `Fix`, `Ask`, and `Verify` in the TUI and WebUI, plus the CLI's `Replan` entry. Other menu actions such as `Generate`, `Approve`, `Run`, and `Publish` are outside this subset. The modeled views are idle: active runs, active fixes, and other in-progress operations are excluded. The CLI assumes no PR target, since `available_session_actions` omits `Replan` when one exists.

| Phase and plan state | TUI | WebUI | CLI |
| --- | --- | --- | --- |
| Awaiting Approval, usable plan | `Fix`, `Ask`, `Verify` | `Fix`, `Ask`, `Verify`; pending prompt hides `Verify` | none |
| Awaiting Approval, available plan with `plan_error` | `Fix` | `Fix` | none |
| Awaiting Approval, no available plan | `Fix` | none | none |
| Planned, usable plan | `Fix`, `Ask`, `Verify` | `Fix`, `Ask`, `Verify`; pending prompt hides `Verify` | `Replan` |
| Planned, unusable plan | none | none | `Replan` |
| Other phases | none | none | none |

The TUI's `usablePlan` is `plan_available && plan_error.is_none()`: `has_usable_plan` and the WebUI's `session_plan_available` use the same markdown-availability helper. The WebUI uses `plan_available` directly for Awaiting Approval, so Fix remains visible when `plan_error` is set; Ask and Verify are hidden. For Planned, it requires an available plan and no error. Its pending-prompt guard suppresses Verify. The CLI exposes Replan in Planned regardless of plan availability or `plan_error`.

## Rust mapping

| Model symbol | Rust behavior represented |
| --- | --- |
| `tuiOffer` / `tuiActions` | `CruiseApplication::capabilities` and `add_approval_actions` in `src/application.rs`; the usable-plan value includes the `plan_error` and file-usability checks |
| `webuiOffer` / `webuiActions` | `show_fix`, `show_ask`, and `show_verify` in `src/webui/actions.rs`, including `plan_available`, `plan_error`, and pending-prompt conditions |
| `cliOffer` / `cliActions` | `available_session_actions` in `src/list_cmd.rs`; Replan is listed only in Planned when no PR target exists |
| `PlanningOffersMatchContract` / `safety` | Internal consistency of the finite action-offer model; not a check against Rust |
| `planning_operation_allowed` | Phase eligibility only, in `src/application.rs`; usable-plan gating for visible actions lives in the client capability/action construction described above |

The BDD in `tests/plan_revision_bdd.rs` checks client parity, phase preservation, and prompt delivery against production, with negative controls that demonstrate those checks can fail. These production-connected checks, not the formal model, are the evidence for those behaviors.

## Limits

The formal model is a finite action-offer table, not a Rust refinement proof. It omits non-review menu actions, actual plan contents, prompt construction, request execution, error transport, active-operation states, and PR-target-specific CLI offers. It assumes the supplied usability and availability flags reflect the corresponding client inputs. The BDD covers production behavior separately; bounded model checking does not prove unbounded behavior.
