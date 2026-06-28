/**
 * @name Audit required: every `unsafe` block must justify itself in code review
 * @description Flags every `unsafe { ... }` block expression in Rust source so
 *              that a reviewer must justify it inline at the call site.  The
 *              Rust API guidelines recommend a `// SAFETY: <reason>` comment
 *              documenting the safety precondition; this query surfaces every
 *              unsafe block as a non-blocking PR annotation
 *              (`@problem.severity recommendation`) so the audit nag never
 *              silently passes review.
 * @kind problem
 * @id rust/audit-unsafe-blocks
 * @problem.severity recommendation
 * @precision medium
 * @tags audit security correctness rust
 */

import rust

from BlockExpr b
where b.isUnsafe()
select b, "audit-required: each `unsafe { ... }` block must have a `// SAFETY: <reason>` comment justifying the safety precondition, per Rust API guidelines"
