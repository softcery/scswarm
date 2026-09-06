---
description: Distill a system into a ref under ./docs/refs/. Architecture and rules only. No code dumps, no plan refs, no obvious stuff.
---

Write a ref for the system: $ARGUMENTS. One tight doc under `./docs/refs/` that locks shape, invariants, rules. A ref is a fence, not a tour. You're the ingenious software architect with IQ of 180.

---

## Protocol

### 1. Restate

Restate system and boundary. In / out. Confirm before §2.

### 2. Review refs

Read `./docs/refs/architecture.md` and any overlapping ref. No duplication. No contradiction. Extend existing or carve fresh — state the choice.

### 3. Map terrain

`rmap` the system. Read load-bearing files only. Note: boundary, public surface, event/effect shape, who reads, who writes, who folds.

### 4. Distill invariants

Each rule = property enforced by types or one chokepoint. "By convention" → surface as risk. Rule must answer: _what breaks on violation._

### 5. Distill pillars

Load-bearing shapes. Signatures, key state fields, chokepoints that make the system inevitable. Signature-level, not body-level.

### 6. Cut

Kill: obvious-from-code, function-body restatement, plan/doc/date/status refs, empty adjectives, examples that pin no rule. If the line prevents no mistake, cut it.

### 7. Write

`./docs/refs/<system>.md`. Order: one-line purpose · Invariants (rule + chokepoint) · Pillars (signature + role) · Authoring recipe (N concrete edits, where) · Smells (patterns of violation). No prose paragraphs >3 sentences. Caveman.

### 8. Cross-link

Tighten a row in `./docs/refs/architecture.md` with a one-line link. Never copy across refs.

### 9. Receipt

Ref path + line count. Invariants / pillars / smells captured. Violations found in code → list as follow-up, do not fix here.

---

Fence not tour.
If it prevents no mistake, cut.
No plan refs, no dates, no statuses.

FULL CAVEMAN MODE ACTIVE.
