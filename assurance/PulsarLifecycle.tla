------------------------ MODULE PulsarLifecycle ------------------------
EXTENDS Naturals, FiniteSets, TLC
CONSTANT MaxRevision
VARIABLES revision, durableRevision, authorized, job, candidateBase,
          committedCount, unauthorizedCommits, deviceArmed, controller,
          sessionEpoch
vars == <<revision, durableRevision, authorized, job, candidateBase,
          committedCount, unauthorizedCommits, deviceArmed, controller,
          sessionEpoch>>
Init ==
  /\ revision = 0
  /\ durableRevision = 0
  /\ authorized = TRUE
  /\ job = "Idle"
  /\ candidateBase = 0
  /\ committedCount = 0
  /\ unauthorizedCommits = 0
  /\ deviceArmed = FALSE
  /\ controller = FALSE
  /\ sessionEpoch = 0
Start ==
  /\ authorized /\ job = "Idle"
  /\ job' = "Running" /\ candidateBase' = revision
  /\ UNCHANGED <<revision,durableRevision,authorized,committedCount,
                 unauthorizedCommits,deviceArmed,controller,sessionEpoch>>
Finish ==
  /\ authorized /\ job = "Running"
  /\ job' = "Candidate"
  /\ UNCHANGED <<revision,durableRevision,authorized,candidateBase,
                 committedCount,unauthorizedCommits,deviceArmed,controller,sessionEpoch>>
Edit ==
  /\ authorized /\ revision < MaxRevision
  /\ revision' = revision + 1 /\ durableRevision' = revision'
  /\ committedCount' = committedCount + 1
  /\ unauthorizedCommits' = unauthorizedCommits + (IF authorized THEN 0 ELSE 1)
  /\ UNCHANGED <<authorized,job,candidateBase,
                 deviceArmed,controller,sessionEpoch>>
Commit ==
  /\ authorized /\ job = "Candidate" /\ candidateBase = revision
  /\ revision < MaxRevision
  /\ revision' = revision + 1 /\ durableRevision' = revision'
  /\ committedCount' = committedCount + 1 /\ job' = "Idle"
  /\ unauthorizedCommits' = unauthorizedCommits + (IF authorized THEN 0 ELSE 1)
  /\ UNCHANGED <<authorized,candidateBase,
                 deviceArmed,controller,sessionEpoch>>
Cancel ==
  /\ job \in {"Running","Candidate"}
  /\ job' = "Cancelled"
  /\ UNCHANGED <<revision,durableRevision,authorized,candidateBase,
                 committedCount,unauthorizedCommits,deviceArmed,controller,sessionEpoch>>
Revoke ==
  /\ authorized
  /\ authorized' = FALSE
  /\ job' = IF job \in {"Running","Candidate"} THEN "Cancelled" ELSE job
  /\ deviceArmed' = FALSE /\ controller' = FALSE
  /\ UNCHANGED <<revision,durableRevision,candidateBase,committedCount,
                 unauthorizedCommits,sessionEpoch>>
Arm ==
  /\ authorized /\ ~deviceArmed
  /\ deviceArmed' = TRUE /\ controller' = TRUE
  /\ UNCHANGED <<revision,durableRevision,authorized,job,candidateBase,
                 committedCount,unauthorizedCommits,sessionEpoch>>
Reconnect ==
  /\ sessionEpoch < 2
  /\ deviceArmed' = FALSE /\ controller' = FALSE
  /\ sessionEpoch' = sessionEpoch + 1
  /\ UNCHANGED <<revision,durableRevision,authorized,job,candidateBase,
                 committedCount,unauthorizedCommits>>
Next == Start \/ Finish \/ Edit \/ Commit \/ Cancel \/ Revoke \/ Arm \/ Reconnect
TypeOK ==
  /\ revision \in 0..MaxRevision
  /\ durableRevision \in 0..MaxRevision
  /\ job \in {"Idle","Running","Candidate","Cancelled"}
  /\ authorized \in BOOLEAN
  /\ deviceArmed \in BOOLEAN
  /\ controller \in BOOLEAN
DurableAcknowledgement == revision = durableRevision
NoUnauthorizedCommit == unauthorizedCommits = 0
AuthorityForArming == deviceArmed => (authorized /\ controller)
Spec == Init /\ [][Next]_vars
=============================================================================
