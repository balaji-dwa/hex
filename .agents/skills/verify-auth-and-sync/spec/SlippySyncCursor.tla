--------------------------- MODULE SlippySyncCursor --------------------------
EXTENDS Integers, FiniteSets

CONSTANTS MaxSeq, Limit, LagRetries
VARIABLES tasks, finished, snapshot, baseline, phase, saved, visible, notified, unread, lag

vars == <<tasks, finished, snapshot, baseline, phase, saved, visible, notified, unread, lag>>
Seqs == 1..MaxSeq
NoCursor == -1
Head == CHOOSE seq \in tasks \cup {0} : \A other \in tasks : other <= seq
Between(start, stop) == {seq \in finished : start < seq /\ seq <= stop}

Init ==
    /\ tasks \in SUBSET Seqs
    \* The first server sequence can only be the first queued task.
    /\ finished \in SUBSET (tasks \ {1})
    /\ snapshot \in SUBSET tasks
    /\ Cardinality(snapshot) =
        IF Cardinality(tasks) < Limit THEN Cardinality(tasks) ELSE Limit
    /\ baseline \in NoCursor..Head
    /\ phase = IF baseline = NoCursor THEN "Unseeded" ELSE "Saved"
    /\ saved = baseline
    /\ visible = {}
    /\ notified = {}
    /\ unread = {}
    /\ lag = 0

Seed ==
    /\ phase = "Unseeded"
    /\ phase' = "Steady"
    /\ saved' = Head
    /\ visible' = snapshot
    /\ UNCHANGED <<tasks, finished, snapshot, baseline, notified, unread, lag>>

LoadSeenHistory ==
    /\ phase = "Saved"
    /\ phase' = "Steady"
    /\ visible' = visible \cup {seq \in snapshot : seq <= saved}
    /\ UNCHANGED <<tasks, finished, snapshot, baseline, saved, notified, unread, lag>>

\* Fold the snapshot delta and positive-cursor pages into ordered visits.
\* A capped zero snapshot switches to the complete positive-cursor scan.
DeltaTasks ==
    IF saved = 0
    THEN IF Cardinality(snapshot) = Limit THEN tasks \ {1} ELSE snapshot
    ELSE {seq \in tasks : seq > saved}

NextSeq == CHOOSE seq \in DeltaTasks : \A other \in DeltaTasks : seq <= other

WaitForDetail ==
    /\ phase # "Unseeded"
    /\ saved < Head
    /\ DeltaTasks # {}
    /\ NextSeq \in finished
    /\ lag < LagRetries
    /\ lag' = lag + 1
    /\ UNCHANGED <<tasks, finished, snapshot, baseline, phase, saved, visible, notified, unread>>

ApplyDelta ==
    /\ phase # "Unseeded"
    /\ saved < Head
    /\ LET seq == IF DeltaTasks = {} THEN Head ELSE NextSeq
           post == IF seq \in finished \ visible THEN {seq} ELSE {}
       IN /\ saved' = seq
          /\ visible' = visible \cup {seq}
          /\ notified' = notified \cup post
          /\ unread' = unread \cup post
    /\ lag' = 0
    /\ UNCHANGED <<tasks, finished, snapshot, baseline, phase>>

Next == Seed \/ LoadSeenHistory \/ WaitForDetail \/ ApplyDelta
Spec == Init /\ [][Next]_vars /\ WF_vars(Seed \/ LoadSeenHistory \/ ApplyDelta)

TypeOK ==
    /\ phase \in {"Unseeded", "Saved", "Steady"}
    /\ saved \in NoCursor..Head
    /\ visible \subseteq tasks
    /\ notified \subseteq finished
    /\ unread \subseteq notified
    /\ lag \in 0..LagRetries

NoSwallowedCompletionOnRestart ==
    baseline # NoCursor => notified = Between(baseline, saved) /\ unread = notified

FirstSeedNoHistorySpam ==
    baseline = NoCursor /\ phase = "Steady" =>
        saved = Head /\ visible = snapshot /\ notified = {} /\ unread = {}

CatchUpReachesServerCursor == <>(phase = "Steady" /\ saved = Head)
=============================================================================
