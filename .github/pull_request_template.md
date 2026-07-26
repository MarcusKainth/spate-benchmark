<!--
The most valuable pull request this repository can receive is one that makes a
competitor faster. If you maintain, or merely know, one of these systems and
think its arm is configured badly, that is a bug — thank you for opening this.

Delete whichever sections do not apply.
-->

## What this changes

<!-- Which arm, or which part of the harness, site or contract. -->

## What you expect it to do to the number, and why

<!--
The most useful field on this form, and the one CONTRIBUTING.md asks for
explicitly. A prediction that turns out wrong is still useful — it usually means
the benchmark is measuring something other than what we thought.

Re-measurement happens after merge, on the reference hardware, because a number
is only comparable to the others if it came off the same machine under the same
protocol. Whatever comes back gets reported against what you predicted here,
including when it does not move.
-->

## The two rules

<!-- For a change to an arm. Both are from methodology/. -->

- [ ] **Rule 1** — this uses the best API the system ships and does not
      hand-write its internals. Configuration tuning is unlimited and expected;
      replacing a system's own deserializer with one we wrote is not, even when
      it wins.
- [ ] **Rule 2** — this optimises hard within rule 1. A slow competitor arm is a
      bug in this benchmark, not a result.
- [ ] Anything the system cannot express is declared in the descriptor rather
      than departed from quietly.

## Checks

- [ ] `bench validate` passes. It is what CI runs, it reports every problem
      rather than the first, and it also checks that every environment profile
      and every committed result still parses.
- [ ] No file under `results/` is added, edited or deleted by this PR. The
      archive is append-only and is not something a pull request edits — a number
      later found to be wrong is corrected by the maintainer in a commit of its
      own.
- [ ] Commits follow [Conventional Commits](https://www.conventionalcommits.org).

## Anything else

<!--
If you think we handicapped your system and this PR is the evidence, say so
plainly. That is the argument this benchmark exists to be able to lose.
-->
