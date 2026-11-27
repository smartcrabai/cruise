#!/usr/bin/env bash
# Run every Manager specification and keep independent evidence, even on failure.
set -uo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1
mkdir -p target/formal-reports
RUN=$(mktemp -d "$ROOT/target/formal-reports/manager.XXXXXX")
status=0
run_check() {
  local name=$1
  shift
  echo "== $name"
  printf '%q ' "$@" >> "$RUN/commands.log"
  printf '\n' >> "$RUN/commands.log"
  "$@" > "$RUN/$name.log" 2>&1
  local result=$?
  cat "$RUN/$name.log"
  printf '%s: %s\n' "$name" "$result" >> "$RUN/results.log"
  if [[ $result != 0 ]]; then status=1; fi
}
run_check setup bash scripts/setup_formal.sh
if [[ $status != 0 || ! -f target/formal-tools/env.sh ]]; then
  echo 'BLOCKED: pinned formal toolchain setup failed' | tee -a "$RUN/results.log"
  exit 1
fi
# shellcheck disable=SC1091
source target/formal-tools/env.sh
Q="$ROOT/tests/formal/node_modules/.bin/quint"
MODEL=tests/formal/manager.qnt
run_check node-version node --version
run_check java-version java -version
run_check quint-version "$Q" --version
run_check rust-version rustc +1.99.1 --version
run_check alloy-version java -jar target/formal-tools/alloy-6.2.0.jar version
run_check alloy-exec-help java -jar target/formal-tools/alloy-6.2.0.jar help exec
require_tests() {
  local log=$1 test
  shift
  for test in "$@"; do
    if ! grep -Fq "ok $test passed" "$log"; then
      echo "Missing Quint test result: $test" | tee -a "$RUN/results.log"
      status=1
    fi
  done
}
run_check quint-typecheck "$Q" typecheck "$MODEL"
run_check quint-test "$Q" test "$MODEL" --backend typescript --seed 20261120
require_tests "$RUN/quint-test.log" approvalAuthorizesAsynchronousSchedulingTest staleApprovalTest dependencyAndPublicationTest pauseResumeTest retryVerificationTest correctiveApprovalTest staleCorrectiveTest correctivePreviewDoesNotAuthorizeTest discardedCorrectiveTest freshCorrectiveAfterRetryTest rejectedRetryPreservesPreviewTest
run_check quint-simulation "$Q" run --seed 20261120 --max-samples 1000 --max-steps 20 --backend typescript --invariant safety "$MODEL"
run_check quint-verify node scripts/formal/verify_quint.cjs "$Q" "$MODEL" manager 16 "$RUN/counterexample.itf.json"
RESERVATION=tests/formal/manager_reservation.qnt
run_check reservation-typecheck "$Q" typecheck "$RESERVATION"
run_check reservation-test "$Q" test "$RESERVATION" --backend typescript --seed 20261120
require_tests "$RUN/reservation-test.log" siblingIntegrationRepinsTest legacyBlockRecoversTest recoveredReservationLaunchesOnceTest retryAfterLaunchIsRejectedTest retryAfterLaunchDoesNotReserveTest publishedStaleStaysBlockedTest publishedStaleDoesNotReactivateTest restartAfterRepinTest restartAfterPublishTest
run_check reservation-simulation "$Q" run --seed 20261120 --max-samples 1000 --max-steps 20 --backend typescript --invariant safety "$RESERVATION"
run_check reservation-verify node scripts/formal/verify_quint.cjs "$Q" "$RESERVATION" manager_reservation 12 "$RUN/reservation-counterexample.itf.json"
CORRECTIONS=tests/formal/manager_corrections.qnt
run_check corrections-typecheck "$Q" typecheck "$CORRECTIONS"
run_check corrections-test "$Q" test "$CORRECTIONS" --backend typescript --seed 20261120
require_tests "$RUN/corrections-test.log" correctedResumeLaunchesOnceTest approvalIsSpentTest launchIsNotRepeatedTest movedTipVoidsResumeTest editedWorktreeVoidsResumeTest otherAttemptVoidsResumeTest plainResumeDeliversNothingTest changeBeforeLaunchIsRefusedTest refusedCorrectionBlocksPlainResumeTest refusedCorrectionIsReviewedAgainTest armedCorrectionSurvivesRestartTest startedCorrectionIsNotReplayedTest blockedGoalLaunchesNothingTest amendmentKeepsProposalAndPlanTest amendmentIsApprovedOnceTest movedTipVoidsAmendmentTest laterAttemptUsesAmendmentTest evidenceResolvesAndFinalTipIsVerifiedTest partialEvidenceIsNotAResultTest redEvidenceLeavesTaskFailedTest unresolvedTaskBlocksDependentsTest interruptedEvidenceIsNotReplayedTest movedTipEvidenceIsNotAdoptedTest amendedContractEvidenceIsNotAdoptedTest movedTipVoidsEvidenceApprovalTest otherAttemptVoidsEvidenceApprovalTest
run_check corrections-simulation "$Q" run --seed 20261120 --max-samples 1000 --max-steps 20 --backend typescript --invariant safety "$CORRECTIONS"
run_check corrections-verify node scripts/formal/verify_quint.cjs "$Q" "$CORRECTIONS" manager_corrections 12 "$RUN/corrections-counterexample.itf.json"
run_check manager-goal-projection-typecheck "$Q" typecheck tests/formal/manager_goal_projection.qnt
run_check manager-goal-projection-test "$Q" test tests/formal/manager_goal_projection.qnt --backend typescript --seed 61
run_check manager-image-binding-typecheck "$Q" typecheck tests/formal/manager_image_binding.qnt
run_check manager-image-binding-test "$Q" test tests/formal/manager_image_binding.qnt --backend typescript --seed 61
QUINT_HANDOFF=tests/formal/manager_quint_file.qnt
run_check manager-quint-file-typecheck "$Q" typecheck "$QUINT_HANDOFF"
run_check manager-quint-file-test "$Q" test "$QUINT_HANDOFF" --backend typescript --seed 61
require_tests "$RUN/manager-quint-file-test.log" exactModelReachesChildByReferenceTest unapprovedSourceCannotReadTest invalidatedSourceCannotBindTest staleProposalCannotBindTest anotherGoalOrTaskCannotReadTest anotherRepoCannotReadTest invalidationBetweenApprovalAndDispatchStopsChildTest changedBytesStopChildTest
run_check alloy java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/manager.als
run_check alloy-receipt node scripts/formal/check_alloy_receipt.cjs tests/formal/manager.als "$RUN/alloy/receipt.json"
run_check receipt-validator-tests node --test scripts/formal/check_alloy_receipt.test.cjs
run_check manager-bdd cargo +1.99.1 test --locked --all-features --test manager_approval_bdd
run_check manager-cli cargo +1.99.1 test --locked --all-features --test manager_corrective_approval
run_check manager-publication cargo +1.99.1 test --locked --all-features --test manager_verification_retry
run_check manager-provider-projection cargo +1.99.1 test --locked --all-features --test manager_conversation_restart_acceptance completed_goals_are_summarized_only_in_provider_context
run_check manager-image-binding cargo +1.99.1 test --locked --all-features --test manager_conversation_restart_acceptance manager_images::
run_check manager-webui cargo +1.99.1 test --locked --all-features --lib webui::tests::manager::manager_retry
run_check manager-webui-render-evidence grep -Fx \
  'test webui::tests::manager::manager_retry_distinguishes_task_from_verification_scope ... ok' \
  "$RUN/manager-webui.log"
printf 'Aggregate exit status: %s\n' "$status" | tee -a "$RUN/results.log"
echo "Manager specification reports: $RUN"
exit "$status"
