---------------------- MODULE PulsarPackageExport ----------------------
EXTENDS Naturals, Integers, TLC
CONSTANT Fault
VARIABLE s
vars == <<s>>
Guard(name) == Fault # name
Active == {"Queued", "Capturing", "Streaming", "Published"}
Terminal == {"Cancelled", "Interrupted", "Failed", "Released"}
Init ==
  s = [phase |-> "None", revision |-> 0, expected |-> 0, captured |-> -1,
       authorized |-> TRUE, authorityEpoch |-> 0, fenced |-> FALSE, epoch |-> 0, sourcePresent |-> TRUE,
       sourceHeld |-> FALSE, workerActive |-> FALSE, reserved |-> FALSE,
       bundlePresent |-> FALSE, bundleDurable |-> FALSE, bundleHeld |-> FALSE,
       readyRecorded |-> FALSE, verification |-> "Absent", verifiedEpoch |-> -1,
       verificationActive |-> FALSE, verificationAuthority |-> -1, verificationValid |-> TRUE, leaseAuthority |-> -1,
       leaseEpoch |-> -1, reading |-> FALSE, writing |-> FALSE, readEpoch |-> -1, cursor |-> 0,
       requestSeen |-> FALSE, terminalSeen |-> FALSE,
       readyAuthority |-> TRUE, admissionValid |-> TRUE, admitted |-> 0]
Start ==
  /\ s.phase = "None" /\ s.authorized
  /\ s' = [s EXCEPT !.phase = "Queued", !.requestSeen = TRUE, !.reserved = TRUE]
Edit ==
  /\ s.revision < 2
  /\ s' = [s EXCEPT !.revision = @ + 1]
Capture ==
  /\ s.phase = "Queued" /\ s.authorized /\ s.sourcePresent
  /\ (~Guard("capture") \/ s.revision = s.expected)
  /\ s' = [s EXCEPT !.phase = "Capturing", !.captured = s.revision,
                    !.sourceHeld = TRUE, !.workerActive = TRUE]
RejectStale ==
  /\ s.phase = "Queued" /\ s.revision # s.expected
  /\ s' = [s EXCEPT !.phase = "Failed", !.terminalSeen = TRUE]
Stream ==
  /\ s.phase = "Capturing" /\ s.authorized
  /\ s' = [s EXCEPT !.phase = "Streaming"]
EvictSource ==
  /\ s.sourcePresent
  /\ (~Guard("source_hold") \/ ~s.sourceHeld)
  /\ s' = [s EXCEPT !.sourcePresent = FALSE]
PublishBytes ==
  /\ s.phase = "Streaming"
  /\ s' = [s EXCEPT !.phase = "Published", !.bundlePresent = TRUE,
                    !.bundleDurable = FALSE]
SyncNamespace ==
  /\ s.phase = "Published" /\ ~s.bundleDurable
  /\ s' = [s EXCEPT !.bundleDurable = TRUE]
RecordReady ==
  /\ s.phase = "Published"
  /\ (~Guard("publication") \/ (s.authorized /\ ~s.fenced))
  /\ (~Guard("durability") \/ s.bundleDurable)
  /\ s' = [s EXCEPT !.phase = "Ready", !.readyRecorded = TRUE,
                    !.readyAuthority = s.authorized /\ ~s.fenced, !.workerActive = FALSE,
                    !.sourceHeld = FALSE, !.reserved = FALSE,
                    !.verifiedEpoch = s.epoch, !.verification = "Verified",
                    !.verificationAuthority = s.authorityEpoch]
Cancel ==
  /\ s.phase \in Active
  /\ s' = [s EXCEPT !.phase = "Cancelled", !.terminalSeen = TRUE, !.fenced = TRUE,
                    !.reserved = IF Guard("reservation") THEN @ ELSE FALSE]
WorkerExit ==
  /\ s.phase \in Terminal /\ (s.workerActive \/ s.reserved \/ s.sourceHeld)
  /\ s' = [s EXCEPT !.workerActive = FALSE, !.sourceHeld = FALSE,
                    !.reserved = FALSE]
Revoke ==
  /\ s.authorized /\ s.authorityEpoch < 1
  /\ s' = [s EXCEPT !.authorized = FALSE, !.authorityEpoch = @ + 1,
                    !.fenced = @ \/ s.phase \in Active]
Grant ==
  /\ ~s.authorized
  /\ s' = [s EXCEPT !.authorized = TRUE]
Restart ==
  /\ s.epoch < 1
  /\ s' = [s EXCEPT !.epoch = @ + 1,
                    !.phase = IF s.phase \in Active THEN "Interrupted" ELSE @,
                    !.terminalSeen = @ \/ s.phase \in Active,
                    !.sourceHeld = FALSE, !.workerActive = FALSE,
                    !.reserved = FALSE, !.reading = FALSE, !.writing = FALSE, !.bundleHeld = FALSE,
                    !.readEpoch = -1, !.leaseEpoch = -1,
                    !.verification = "Absent", !.verifiedEpoch = -1,
                    !.verificationActive = FALSE]
VerifyStart ==
  /\ s.phase = "Ready" /\ s.authorized /\ s.bundlePresent
  /\ ~s.verificationActive
  /\ (s.verification = "Absent" \/ s.verificationAuthority # s.authorityEpoch)
  /\ s' = [s EXCEPT !.verification = "Verifying", !.verificationActive = TRUE,
                    !.verificationAuthority = s.authorityEpoch]
VerifyFinish ==
  /\ s.phase = "Ready" /\ s.authorized /\ s.bundlePresent
  /\ s.verification = "Verifying"
  /\ (~Guard("verification_generation") \/ s.verificationAuthority = s.authorityEpoch)
  /\ s' = [s EXCEPT !.verification = "Verified", !.verifiedEpoch = s.epoch,
                    !.verificationActive = FALSE,
                    !.verificationValid = @ /\ s.verificationAuthority = s.authorityEpoch]
VerifyAbandon ==
  /\ s.verificationActive
  /\ (~s.authorized \/ s.verificationAuthority # s.authorityEpoch \/ s.phase # "Ready")
  /\ s' = [s EXCEPT !.verificationActive = FALSE, !.verification = "Absent",
                    !.verifiedEpoch = -1]
Lease ==
  /\ s.phase = "Ready" /\ s.authorized /\ s.bundlePresent /\ s.leaseEpoch = -1
  /\ s.verifiedEpoch = s.epoch
  /\ (~Guard("completion") \/ s.verification = "Verified")
  /\ s.verificationAuthority = s.authorityEpoch
  /\ s' = [s EXCEPT !.leaseEpoch = s.epoch, !.leaseAuthority = s.authorityEpoch]
CloseLease ==
  /\ s.leaseEpoch >= 0 /\ ~s.reading /\ ~s.writing
  /\ s' = [s EXCEPT !.leaseEpoch = -1]
ReadStart ==
  /\ s.phase = "Ready" /\ s.authorized /\ s.bundlePresent
  /\ s.leaseEpoch = s.epoch /\ ~s.reading /\ ~s.writing /\ s.admitted < 2
  /\ \E requestEpoch \in 0..s.epoch:
       s' = [s EXCEPT !.reading = TRUE, !.bundleHeld = TRUE, !.readEpoch = requestEpoch]
AdmitChunk ==
  /\ s.reading
  /\ (~Guard("chunk") \/ (s.authorized /\ s.phase = "Ready" /\
                          s.leaseEpoch = s.epoch /\ (~Guard("epoch") \/ s.readEpoch = s.epoch) /\
                          (~Guard("authority_generation") \/ s.leaseAuthority = s.authorityEpoch)))
  /\ s' = [s EXCEPT !.reading = FALSE, !.writing = TRUE, !.readEpoch = -1,
                    !.admitted = @ + 1,
                    !.admissionValid = @ /\ s.authorized /\ s.phase = "Ready"
                       /\ s.leaseEpoch = s.epoch /\ s.readEpoch = s.epoch
                       /\ s.leaseAuthority = s.authorityEpoch]
WriteFinish ==
  /\ s.writing
  /\ s' = [s EXCEPT !.writing = FALSE, !.bundleHeld = FALSE, !.cursor = @ + 1]
WriteFailure ==
  /\ s.writing
  /\ s' = [s EXCEPT !.writing = FALSE, !.bundleHeld = FALSE]
DiscardRead ==
  /\ s.reading
  /\ s' = [s EXCEPT !.reading = FALSE, !.bundleHeld = FALSE, !.readEpoch = -1]
Release ==
  /\ s.phase = "Ready" /\ s.authorized
  /\ s' = [s EXCEPT !.phase = "Released", !.terminalSeen = TRUE,
                    !.leaseEpoch = -1, !.verification = "Absent",
                    !.verifiedEpoch = -1]
DeleteReleased ==
  /\ s.phase = "Released" /\ s.bundlePresent
  /\ (~Guard("bundle_hold") \/ (~s.bundleHeld /\ ~s.verificationActive))
  /\ s' = [s EXCEPT !.bundlePresent = FALSE, !.bundleDurable = FALSE]
Replay ==
  /\ s.requestSeen /\ s.phase \in Terminal /\ s.authorized
  /\ s' = IF Guard("replay") THEN s
          ELSE [s EXCEPT !.phase = "Queued", !.reserved = TRUE]
Next == Start \/ Edit \/ Capture \/ RejectStale \/ Stream \/ EvictSource
        \/ PublishBytes \/ SyncNamespace \/ RecordReady \/ Cancel \/ WorkerExit \/ Revoke
        \/ Grant \/ Restart \/ VerifyStart \/ VerifyFinish \/ VerifyAbandon \/ Lease \/ CloseLease
        \/ ReadStart \/ AdmitChunk \/ WriteFinish \/ WriteFailure \/ DiscardRead \/ Release
        \/ DeleteReleased \/ Replay
TypeOK ==
  /\ s.phase \in {"None", "Queued", "Capturing", "Streaming", "Published",
                  "Ready", "Cancelled", "Interrupted", "Failed", "Released"}
  /\ s.revision \in 0..2 /\ s.captured \in -1..2
  /\ s.authorityEpoch \in 0..1
  /\ s.epoch \in 0..1 /\ s.leaseEpoch \in -1..1 /\ s.verifiedEpoch \in -1..1
  /\ s.admitted \in 0..2 /\ s.cursor \in 0..s.admitted
SnapshotExact == s.captured >= 0 => s.captured = s.expected
SourceRetention == s.workerActive =>
  (s.sourceHeld /\ s.sourcePresent /\ s.reserved)
ReadyIsDurable == s.phase = "Ready" =>
  (s.readyRecorded /\ s.bundlePresent /\ s.bundleDurable /\ s.readyAuthority)
LeaseIsVerified == (s.leaseEpoch >= 0 /\ s.leaseAuthority = s.authorityEpoch) =>
  (s.verifiedEpoch = s.epoch /\ s.verification = "Verified")
ReadRetainsBundle ==
  /\ ((s.reading \/ s.writing) => s.bundleHeld)
  /\ ((s.reading \/ s.writing \/ s.verificationActive) => s.bundlePresent)
AuthorizedAdmission == s.admissionValid
NoStaleVerification == s.verificationValid
NoReplayResurrection == s.terminalSeen => s.phase \in Terminal
Spec == Init /\ [][Next]_vars
=============================================================================
