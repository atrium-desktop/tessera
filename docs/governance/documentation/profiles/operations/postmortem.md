# Entity: Incident Postmortem

Intent: Conduct blameless post-incident forensics to identify systemic defense failures, detection gaps, and binding preventative action items.

---

## 1. Intent & Escalation Scope

Postmortems capture forensic truth following outages, data corruption, or severe SLA degradations. Operational knowledge escalates as follows:
- **Mitigation**: Immediate restoration steps belong in operational runbooks (`docs/dev/runbooks/`).
- **Forensics**: Systematic timeline and root-cause analysis belong in postmortems (`docs/dev/postmortems/`).
- **Architectural Ratification**: If preventing recurrence requires structural protocol or boundary redesign, an ADR must be authored under `docs/adr/`.

---

## 2. Normative Invariants

- **`[INV-AGENT-03]` Blameless Postmortem Structure**:
  Post-incident reviews must focus strictly on timeline reconstruction, detection gaps, and systemic defense failures. Attribution of personal human error, negligence, or blame is strictly prohibited.
- **`[INV-LINT-02]` Contributor Firewall**:
  Postmortems reside under `docs/dev/postmortems/` behind the contributor firewall. Public user-facing documentation must never contain relative links into postmortems.
- **Permanent Cold Retention**:
  Postmortems are permanent cold-tier forensic records. They must never be deleted or truncated.

---

## 3. Directory Setup & File Naming

Postmortems reside in cold storage under `docs/dev/postmortems/`:
- **File Naming**: `docs/dev/postmortems/YYYY-MM-DD-<slug>.md` (e.g., `2026-04-12-wal-corruption.md`).
- **Registry Index**: Maintain a chronological registry at `docs/dev/postmortems/index.md`.

---

## 4. Authoritative Postmortem Template

```markdown
# Postmortem: [Incident Title]

- Date of Incident: YYYY-MM-DD
- Date Published: YYYY-MM-DD
- Severity: Sev 1 | Sev 2 | Sev 3
- Lead Investigator: [Name / GitHub handle]
- Impact Summary: [e.g., 42 minutes degraded throughput; zero permanent data loss]

---

## 1. Executive Summary

A 2–3 sentence overview of what broke, the user-visible impact, and the key fix deployed.

## 2. Incident Timeline (UTC)

Chronological reconstruction of events from trigger to recovery:
- **HH:MM** - Incident trigger or deployment occurs.
- **HH:MM** - First automated alert fires or customer report received.
- **HH:MM** - Incident response team assembles; mitigation triage begins.
- **HH:MM** - Mitigation deployed (e.g., traffic rerouted, rollback applied).
- **HH:MM** - Recovery verified; incident declared resolved.

## 3. Root Cause Analysis (The 5 Whys)

1. *Why did the service crash?* Buffer overflowed under sudden traffic spike.
2. *Why did the buffer overflow?* Backpressure signaling was disabled on worker threads.
3. *Why was backpressure disabled?* Feature flag introduced during last release had inverted boolean logic.
4. *Why was the inverted logic not caught in CI?* Integration tests ran with backpressure pre-mocked.
5. *Why was it pre-mocked?* Test fixture lacked end-to-end backpressure validation.

## 4. Detection & Response Gaps

- **Time to Detect (TTD)**: [e.g., 14 minutes from trigger to alert].
- **Monitoring Gap**: Alert threshold was configured on 15-minute rolling average rather than instantaneous drop.
- **Response Friction**: Runbook lacked explicit command to drain worker pools safely.

## 5. Preventative Action Items (Systemic Defense)

| Action Item | Type | Owner | Tracking Issue / ADR |
| :--- | :--- | :--- | :--- |
| Add E2E backpressure integration test | Test | @engineer | #412 |
| Rewrite alert rule to trigger on P99 spike | Monitoring | @sre | #415 |
| Document graceful worker pool drain command | Runbook | @maintainer | #418 |
| Evaluate strict typed feature-flag configuration | Architecture | @architect | ADR-0012 |
```
