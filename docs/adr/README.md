# Architecture decision records

An ADR records one large design decision: why it was made, what else was
considered, and what follows from it. Smaller decisions go in the `docs/` page that
owns the topic (see [Recording decisions](../README.md#recording-decisions)).

## Format

Name the file `NNNN-short-title.md`, numbered after the highest existing ADR, and
add it to the list below in the same PR. Start it with:

```markdown
# ADR NNNN: Title

Status: accepted by the owner on YYYY-MM-DD HH:MM UTC ("the owner's words, quoted");
implemented in [#123](https://github.com/risu729/biwa/pull/123) / not yet implemented.
Amends / replaces ADR NNNN, if any.
Date: YYYY-MM-DD
```

The body has Context, Decision, Consequences and Alternatives sections. Mark
anything an agent added beyond the owner's answer as not confirmed by the owner.

After acceptance, change an ADR only through a dated amendment section that keeps
the accepted text, and note the amendment in its `Status:` line. An ADR that amends
another also updates the other's `Status:` line.

## Records

None yet.
