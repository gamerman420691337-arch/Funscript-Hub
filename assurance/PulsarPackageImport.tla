----------------------- MODULE PulsarPackageImport -----------------------
EXTENDS Naturals, TLC
CONSTANT Fault
VARIABLE s
vars == <<s>>
Guard(name) == Fault # name
Active == {"Uploading", "Sealing", "Validating", "Validated", "Publishing"}
Init ==
  s = [phase |-> "None", authenticated |-> TRUE, epoch |-> 0,
       requestSeen |-> FALSE, requestIntent |-> 0, inputIntent |-> 0,
       sealedIntent |-> 0, resultIntent |-> 0, cursor |-> 0, immutable |-> FALSE,
       contentValid |-> TRUE, closureValid |-> TRUE, verified |-> FALSE,
       recoveryDurable |-> FALSE, objectsPublished |-> FALSE,
       objectsDurable |-> FALSE, preexistingPresent |-> TRUE,
       archiveInert |-> TRUE, freshIdentity |-> TRUE,
       reserved |-> FALSE, workerActive |-> FALSE, cancellationFence |-> FALSE,
       visible |-> FALSE, rowsComplete |-> FALSE, creatorGrant |-> FALSE,
       originGrant |-> FALSE, committed |-> FALSE, acknowledged |-> FALSE,
       projectCount |-> 0, terminalSeen |-> FALSE]
Start ==
  /\ s.phase = "None" /\ s.authenticated
  /\ s' = [s EXCEPT !.phase = "Uploading", !.requestSeen = TRUE, !.reserved = TRUE]
Upload ==
  /\ s.phase = "Uploading" /\ s.authenticated /\ s.cursor < 2
  /\ s' = [s EXCEPT !.cursor = @ + 1]
UploadCorrupt ==
  /\ s.phase = "Uploading" /\ s.authenticated /\ s.cursor < 2
  /\ s' = [s EXCEPT !.cursor = @ + 1, !.contentValid = FALSE]
UploadInvalidGraph ==
  /\ s.phase = "Uploading" /\ s.authenticated /\ s.cursor < 2
  /\ s' = [s EXCEPT !.cursor = @ + 1, !.closureValid = FALSE]
Seal ==
  /\ s.phase = "Uploading" /\ s.authenticated
  /\ (~Guard("full_bytes") \/ s.cursor = 2)
  /\ s' = [s EXCEPT !.phase = "Sealing", !.immutable = TRUE, !.workerActive = TRUE,
                    !.sealedIntent = s.inputIntent]
BeginValidation ==
  /\ s.phase = "Sealing"
  /\ s' = [s EXCEPT !.phase = "Validating"]
Validate ==
  /\ s.phase = "Validating" /\ s.authenticated
  /\ (~Guard("content") \/ s.contentValid)
  /\ (~Guard("closure") \/ s.closureValid)
  /\ s' = [s EXCEPT !.phase = "Validated", !.verified = TRUE,
                    !.freshIdentity = Guard("identity")]
RejectInvalid ==
  /\ s.phase = "Validating" /\ (~s.contentValid \/ ~s.closureValid)
  /\ s' = [s EXCEPT !.phase = "Failed", !.terminalSeen = TRUE]
CasBypass ==
  /\ ~Guard("cas_hit") /\ s.phase = "Uploading" /\ s.preexistingPresent
  /\ s' = [s EXCEPT !.phase = "Validated", !.verified = TRUE]
DurableRecovery ==
  /\ s.phase = "Validated" /\ ~s.recoveryDurable
  /\ s' = [s EXCEPT !.recoveryDurable = TRUE]
Publish ==
  /\ s.phase = "Validated"
  /\ (~Guard("recovery") \/ s.recoveryDurable)
  /\ s' = [s EXCEPT !.phase = "Publishing", !.objectsPublished = TRUE]
SyncObjects ==
  /\ s.phase = "Publishing" /\ ~s.objectsDurable
  /\ s' = [s EXCEPT !.objectsDurable = TRUE]
Commit ==
  /\ s.phase = "Publishing"
  /\ (s.recoveryDurable /\ (~Guard("object_sync") \/ s.objectsDurable))
  /\ (~Guard("authorization") \/ (s.authenticated /\ ~s.cancellationFence))
  /\ s' = [s EXCEPT !.phase = "Completed", !.visible = TRUE,
                    !.rowsComplete = Guard("atomic_rows"), !.creatorGrant = TRUE,
                    !.originGrant = ~Guard("origin_authority"),
                    !.archiveInert = Guard("origin_authority"),
                    !.committed = TRUE, !.projectCount = @ + 1,
                    !.resultIntent = s.sealedIntent,
                    !.reserved = FALSE, !.workerActive = FALSE]
Acknowledge ==
  /\ s.phase = "Completed"
  /\ s' = [s EXCEPT !.acknowledged = TRUE]
Cancel ==
  /\ s.phase \in Active
  /\ s' = [s EXCEPT !.phase = "Cancelled", !.cancellationFence = TRUE,
                    !.terminalSeen = TRUE,
                    !.reserved = IF Guard("reservation") THEN @ ELSE FALSE]
InvalidateAuthentication ==
  /\ s.authenticated
  /\ s' = [s EXCEPT !.authenticated = FALSE,
                    !.cancellationFence = @ \/ s.phase \in Active]
RestoreAuthentication ==
  /\ ~s.authenticated
  /\ s' = [s EXCEPT !.authenticated = TRUE]
WorkerExit ==
  /\ s.phase \in {"Cancelled", "Interrupted", "Failed"}
  /\ (s.workerActive \/ s.reserved)
  /\ s' = [s EXCEPT !.workerActive = FALSE, !.reserved = FALSE]
Crash ==
  /\ s.epoch < 1
  /\ s' = [s EXCEPT !.epoch = @ + 1, !.workerActive = FALSE, !.reserved = FALSE,
                    !.phase = IF s.phase \in Active THEN "Interrupted" ELSE @,
                    !.terminalSeen = @ \/ s.phase \in Active]
Cleanup ==
  /\ s.phase \in {"Cancelled", "Interrupted", "Failed"} /\ ~s.workerActive
  /\ s' = [s EXCEPT !.preexistingPresent = Guard("shared_object")]
Replay ==
  /\ s.requestSeen /\ s.authenticated
  /\ s' = IF Guard("replay") THEN s
          ELSE IF s.phase = "Completed" THEN [s EXCEPT !.projectCount = 2]
          ELSE s
ReplayInterrupted ==
  /\ s.requestSeen /\ s.phase \in {"Cancelled", "Interrupted", "Failed"}
  /\ ~Guard("terminal_replay") /\ s.authenticated
  /\ s' = [s EXCEPT !.phase = "Uploading", !.reserved = TRUE]
ChangedIntent ==
  /\ s.requestSeen /\ s.phase = "Uploading" /\ s.authenticated
  /\ ~Guard("request_binding")
  /\ s' = [s EXCEPT !.inputIntent = 1, !.cursor = 0]
PrematureAck ==
  /\ ~Guard("ack") /\ s.phase \in Active
  /\ s' = [s EXCEPT !.acknowledged = TRUE]
Next == Start \/ Upload \/ UploadCorrupt \/ UploadInvalidGraph \/ Seal
        \/ BeginValidation \/ Validate \/ RejectInvalid \/ CasBypass \/ DurableRecovery
        \/ Publish \/ SyncObjects \/ Commit \/ Acknowledge \/ Cancel
        \/ InvalidateAuthentication \/ RestoreAuthentication \/ WorkerExit
        \/ Crash \/ Cleanup \/ Replay \/ ReplayInterrupted \/ ChangedIntent
        \/ PrematureAck
TypeOK ==
  /\ s.phase \in {"None", "Uploading", "Sealing", "Validating", "Validated",
                  "Publishing", "Completed", "Cancelled", "Interrupted", "Failed"}
  /\ s.cursor \in 0..2 /\ s.epoch \in 0..1 /\ s.projectCount \in 0..2
  /\ s.requestIntent \in 0..1 /\ s.inputIntent \in 0..1
  /\ s.sealedIntent \in 0..1 /\ s.resultIntent \in 0..1
VerifiedPlan == s.verified =>
  (s.cursor = 2 /\ s.immutable /\ s.contentValid /\ s.closureValid)
RecoveryBeforeObjectPublication == s.objectsPublished => s.recoveryDurable
DurableVisibility == s.visible =>
  (s.verified /\ s.objectsPublished /\ s.objectsDurable /\ s.recoveryDurable)
AtomicVisibility == s.visible => (s.rowsComplete /\ s.creatorGrant /\ s.committed)
FreshIdentity == s.visible => s.freshIdentity
NoImportedAuthority == ~s.originGrant /\ s.archiveInert
NoCancelledPublication == s.visible => ~s.cancellationFence
RetainedReservation == s.workerActive => s.reserved
SafeAcknowledgement == s.acknowledged => (s.visible /\ s.committed /\ s.rowsComplete)
PreservePreexisting == s.preexistingPresent
AtMostOneProject == s.projectCount <= 1
RequestBinding == s.visible => s.resultIntent = s.requestIntent
NoTerminalResurrection == s.terminalSeen =>
  s.phase \in {"Cancelled", "Interrupted", "Failed"}
Spec == Init /\ [][Next]_vars
=============================================================================
