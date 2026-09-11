----------------------- MODULE PulsarTransfers -----------------------
EXTENDS Naturals, FiniteSets, TLC
CONSTANTS MaxBytes, MaxRevision, MaxEpoch
Actors == {"owner", "other"}
VARIABLES revision, durableRevision, baseRevision, epoch, leaseEpoch,
          state, received, digest, authorized, connected, reserved,
          unauthorizedEffects, staleEpochEffects, badDigestPublications,
          staleCommits
vars == <<revision, durableRevision, baseRevision, epoch, leaseEpoch,
          state, received, digest, authorized, connected, reserved,
          unauthorizedEffects, staleEpochEffects, badDigestPublications,
          staleCommits>>
CanAuthor(actor) == actor = "owner" /\ authorized
Active == state \in {"Receiving", "Received"}
Init ==
  /\ revision = 0 /\ durableRevision = 0 /\ baseRevision = 0
  /\ epoch = 0 /\ leaseEpoch = 0
  /\ state = "Idle" /\ received = 0 /\ digest = "Unchecked"
  /\ authorized = TRUE /\ connected = TRUE /\ reserved = FALSE
  /\ unauthorizedEffects = 0 /\ staleEpochEffects = 0
  /\ badDigestPublications = 0 /\ staleCommits = 0
Begin(actor) ==
  /\ state \in {"Idle", "Abandoned", "Lost", "Committed"}
  /\ connected /\ CanAuthor(actor)
  /\ state' = "Receiving" /\ received' = 0 /\ digest' = "Unchecked"
  /\ baseRevision' = revision /\ leaseEpoch' = epoch /\ reserved' = TRUE
  /\ UNCHANGED <<revision, durableRevision, epoch, authorized, connected,
                  unauthorizedEffects, staleEpochEffects,
                  badDigestPublications, staleCommits>>
AppendChunk(actor, tokenEpoch) ==
  /\ state = "Receiving" /\ connected /\ received < MaxBytes
  /\ CanAuthor(actor) \* MUTATION_WRITE_AUTH
  /\ tokenEpoch = leaseEpoch /\ leaseEpoch = epoch \* MUTATION_EPOCH
  /\ received' = received + 1
  /\ state' = IF received' = MaxBytes THEN "Received" ELSE "Receiving"
  /\ unauthorizedEffects' = unauthorizedEffects +
       (IF CanAuthor(actor) THEN 0 ELSE 1)
  /\ staleEpochEffects' = staleEpochEffects +
       (IF tokenEpoch = leaseEpoch /\ leaseEpoch = epoch THEN 0 ELSE 1)
  /\ UNCHANGED <<revision, durableRevision, baseRevision, epoch, leaseEpoch,
                  digest, authorized, connected, reserved,
                  badDigestPublications, staleCommits>>
VerifyDigest(matches) ==
  /\ state = "Received" /\ digest = "Unchecked"
  /\ digest' = IF matches THEN "Valid" ELSE "Invalid"
  /\ UNCHANGED <<revision, durableRevision, baseRevision, epoch, leaseEpoch,
                  state, received, authorized, connected, reserved,
                  unauthorizedEffects, staleEpochEffects,
                  badDigestPublications, staleCommits>>
Finalize(actor) ==
  /\ state = "Received" /\ connected /\ CanAuthor(actor)
  /\ leaseEpoch = epoch
  /\ digest = "Valid" \* MUTATION_DIGEST
  /\ state' = "Candidate" /\ reserved' = FALSE
  /\ badDigestPublications' = badDigestPublications +
       (IF digest = "Valid" THEN 0 ELSE 1)
  /\ UNCHANGED <<revision, durableRevision, baseRevision, epoch, leaseEpoch,
                  received, digest, authorized, connected,
                  unauthorizedEffects, staleEpochEffects, staleCommits>>
Commit(actor) ==
  /\ state = "Candidate" /\ connected /\ CanAuthor(actor)
  /\ revision < MaxRevision
  /\ baseRevision = revision \* MUTATION_REVISION
  /\ revision' = revision + 1 /\ durableRevision' = revision'
  /\ state' = "Committed"
  /\ staleCommits' = staleCommits +
       (IF baseRevision = revision THEN 0 ELSE 1)
  /\ UNCHANGED <<baseRevision, epoch, leaseEpoch, received, digest,
                  authorized, connected, reserved, unauthorizedEffects,
                  staleEpochEffects, badDigestPublications>>
ConcurrentEdit ==
  /\ authorized /\ revision < MaxRevision
  /\ revision' = revision + 1 /\ durableRevision' = revision'
  /\ UNCHANGED <<baseRevision, epoch, leaseEpoch, state, received, digest,
                  authorized, connected, reserved, unauthorizedEffects,
                  staleEpochEffects, badDigestPublications, staleCommits>>
Revoke ==
  /\ authorized /\ authorized' = FALSE
  /\ state' = IF Active THEN "Abandoned" ELSE state
  /\ reserved' = FALSE
  /\ UNCHANGED <<revision, durableRevision, baseRevision, epoch, leaseEpoch,
                  received, digest, connected, unauthorizedEffects,
                  staleEpochEffects, badDigestPublications, staleCommits>>
AbandonOrExpire ==
  /\ Active /\ state' = "Abandoned" /\ reserved' = FALSE
  /\ UNCHANGED <<revision, durableRevision, baseRevision, epoch, leaseEpoch,
                  received, digest, authorized, connected, unauthorizedEffects,
                  staleEpochEffects, badDigestPublications, staleCommits>>
Disconnect ==
  /\ connected /\ connected' = FALSE
  /\ UNCHANGED <<revision, durableRevision, baseRevision, epoch, leaseEpoch,
                  state, received, digest, authorized, reserved,
                  unauthorizedEffects, staleEpochEffects,
                  badDigestPublications, staleCommits>>
Reconnect ==
  /\ ~connected /\ connected' = TRUE
  /\ UNCHANGED <<revision, durableRevision, baseRevision, epoch, leaseEpoch,
                  state, received, digest, authorized, reserved,
                  unauthorizedEffects, staleEpochEffects,
                  badDigestPublications, staleCommits>>
Restart ==
  /\ epoch < MaxEpoch /\ epoch' = epoch + 1
  /\ state' = IF Active THEN "Lost" ELSE state
  /\ reserved' = FALSE /\ connected' = FALSE
  /\ UNCHANGED <<revision, durableRevision, baseRevision, leaseEpoch,
                  received, digest, authorized, unauthorizedEffects,
                  staleEpochEffects, badDigestPublications, staleCommits>>
Next ==
  \/ (\E actor \in Actors : Begin(actor))
  \/ (\E actor \in Actors, tokenEpoch \in 0..MaxEpoch :
       AppendChunk(actor, tokenEpoch))
  \/ (\E matches \in BOOLEAN : VerifyDigest(matches))
  \/ (\E actor \in Actors : Finalize(actor))
  \/ (\E actor \in Actors : Commit(actor))
  \/ ConcurrentEdit \/ Revoke \/ AbandonOrExpire
  \/ Disconnect \/ Reconnect \/ Restart
TypeOK ==
  /\ revision \in 0..MaxRevision /\ durableRevision \in 0..MaxRevision
  /\ baseRevision \in 0..MaxRevision /\ epoch \in 0..MaxEpoch
  /\ leaseEpoch \in 0..MaxEpoch
  /\ state \in {"Idle", "Receiving", "Received", "Candidate",
                 "Committed", "Abandoned", "Lost"}
  /\ received \in 0..MaxBytes /\ digest \in {"Unchecked", "Valid", "Invalid"}
  /\ authorized \in BOOLEAN /\ connected \in BOOLEAN /\ reserved \in BOOLEAN
  /\ unauthorizedEffects \in Nat /\ staleEpochEffects \in Nat
  /\ badDigestPublications \in Nat /\ staleCommits \in Nat
NoUnauthorizedEffects == unauthorizedEffects = 0
NoStaleEpochEffects == staleEpochEffects = 0
NoBadDigestPublication == badDigestPublications = 0
NoStaleCommit == staleCommits = 0
DurableAcknowledgement == revision = durableRevision
ReservationMatchesLifecycle == reserved = Active
LiveLeaseEpoch == Active => leaseEpoch = epoch
Spec == Init /\ [][Next]_vars
======================================================================
