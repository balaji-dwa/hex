--------------------------- MODULE TeamAuthRefresh ---------------------------
\* A signed-in Card checks its saved session while the Slippy worker runs.
\* A worker auth rejection can arrive while the Card's check is in flight.

VARIABLES auth, check, worker, cursorPresent, cachePresent, outcome

vars == <<auth, check, worker, cursorPresent, cachePresent, outcome>>
TerminalErrors == {"Refresh", "Denied", "Verification"}
Member == auth = "SignedIn"

Init ==
    /\ auth = "SignedIn"
    /\ check = "Scheduled"
    /\ worker = "Running"
    /\ cursorPresent = TRUE
    /\ cachePresent = TRUE
    /\ outcome = "None"

StartCheck ==
    /\ check \in {"Scheduled", "Retry"}
    /\ check' = "InFlight"
    /\ outcome' = "None"
    /\ UNCHANGED <<auth, worker, cursorPresent, cachePresent>>

CompleteCheck(result, schedule) ==
    /\ check = "InFlight"
    /\ check' = schedule
    /\ outcome' = result
    /\ UNCHANGED <<auth, worker, cursorPresent, cachePresent>>

Revoke(error, source) ==
    /\ Member
    /\ source = "Worker" \/ check = "InFlight"
    /\ auth' = "NeedsReauth"
    /\ check' = IF source = "Worker" /\ check = "InFlight" THEN "Stale" ELSE "Idle"
    /\ worker' = "Stopping"
    /\ outcome' = error
    /\ UNCHANGED <<cursorPresent, cachePresent>>

DiscardStaleCheck ==
    /\ check = "Stale"
    /\ check' = "Idle"
    /\ UNCHANGED <<auth, worker, cursorPresent, cachePresent, outcome>>

CleanUp ==
    /\ worker = "Stopping"
    /\ worker' = "Stopped"
    /\ cursorPresent' = FALSE
    /\ cachePresent' = FALSE
    /\ UNCHANGED <<auth, check, outcome>>

Next ==
    \/ StartCheck
    \/ CompleteCheck("Verified", "Scheduled")
    \/ CompleteCheck("Service", "Retry")
    \/ CompleteCheck("Cancelled", "Retry")
    \/ \E error \in TerminalErrors : Revoke(error, "Check")
    \/ Revoke("Refresh", "Worker")
    \/ DiscardStaleCheck
    \/ CleanUp

Spec == Init /\ [][Next]_vars /\ WF_vars(CleanUp)

TypeOK ==
    /\ auth \in {"SignedIn", "NeedsReauth"}
    /\ check \in {"Scheduled", "Retry", "InFlight", "Stale", "Idle"}
    /\ worker \in {"Running", "Stopping", "Stopped"}
    /\ cursorPresent \in BOOLEAN
    /\ cachePresent \in BOOLEAN
    /\ outcome \in TerminalErrors \cup {"None", "Verified", "Service", "Cancelled"}

TransientOutagePreservesSessionAndRetry ==
    outcome = "Service" =>
        Member /\ worker = "Running" /\ cursorPresent /\ cachePresent /\ check = "Retry"

CancellationPreservesSessionAndRetry ==
    outcome = "Cancelled" =>
        Member /\ worker = "Running" /\ cursorPresent /\ cachePresent /\ check = "Retry"

AuthFailureRevokesSession ==
    outcome \in TerminalErrors =>
        auth = "NeedsReauth" /\ check \in {"Stale", "Idle"} /\ worker # "Running"

RevokedMemberIsCleanedUp ==
    (auth = "NeedsReauth") ~> (worker = "Stopped" /\ ~cursorPresent /\ ~cachePresent)
=============================================================================
