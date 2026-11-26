# API Usability Self-Check Criteria

Use this checklist when adding or changing public APIs. Review from the perspective of a **first-time user** who has only the docs and the type signatures.

**Reviewed by:** ______ **Date:** ______ **Version / PR:** ______

---

## 1. Getting Started
- [ ] A first-time user can get a meaningful result within a few minutes.
- [ ] The first README example runs as-is when copy-pasted.
- [ ] Sensible defaults cover the common case without extra configuration.
- [ ] Required boilerplate is minimal.

## 2. Predictability
- [ ] Names describe what the function/class actually does.
- [ ] Naming, argument order, and return types are consistent across similar APIs.
- [ ] Side effects (I/O, global state, mutating inputs) are explicit and documented.
- [ ] The API follows the conventions of the language and ecosystem.
- [ ] The behavior matches what users are likely to expect (mental model).

## 3. Error Prevention
- [ ] Incorrect usage fails early and loudly rather than producing silently wrong results.
- [ ] Type hints / signatures prevent common misuse.
- [ ] Destructive or risky operations require explicit opt-in.
- [ ] Options use enums or explicit keyword arguments instead of magic strings.

## 4. Error Experience
- [ ] Error messages explain what went wrong and how to fix it.
- [ ] Stack traces point to the user's code, not deep internals.
- [ ] Exception types are specific enough to be caught selectively.

## 5. Abstraction Level
- [ ] Simple tasks are simple; complex tasks are still possible.
- [ ] Advanced use extends the basic usage instead of requiring a rewrite.
- [ ] An escape hatch exists (lower-level API, hooks, injectable implementations).
- [ ] Each abstraction has at least one real use case beyond a single implementation.
- [ ] Implementation details do not leak into the public interface.

## 6. Surface Area
- [ ] The number of concepts a user must learn is small.
- [ ] There is one obvious way to do each common task.
- [ ] Public vs. internal boundaries are explicit.
- [ ] No public API exists without a clear user need.

## 7. Feedback & Visibility
- [ ] Users can easily verify that a call did what they intended (return values, state inspection).
- [ ] Long-running or async operations expose progress or status where relevant.

## 8. Documentation
- [ ] Every public API has a docstring covering parameters, return values, raised errors, and side effects.
- [ ] Examples exist for each primary use case.
- [ ] Good examples are not compensating for an awkward API design.

## 9. Testability
- [ ] Code using this package can be tested without network or external services.
- [ ] Dependencies can be mocked or injected.

## 10. Evolution & Compatibility
- [ ] Changes follow semantic versioning.
- [ ] Deprecations emit warnings before removal.
- [ ] Breaking changes include a migration guide.

---

## Validation Methods
- [ ] **README-first:** Usage examples were written before the implementation.
- [ ] **Cognitive walkthrough:** At least one core task was walked through step by step.
- [ ] **User test:** Someone unfamiliar with the package completed a core task using only the docs.
- [ ] **Issue review:** Recurring "how do I…?" questions and misuse patterns were checked.

## Notes
<!-- Record failed items, trade-offs, and follow-up actions here. -->

## References
- Myers & Stylos, "Improving API Usability", *Communications of the ACM*, 2016
- Clarke, "Measuring API Usability", *Dr. Dobb's Journal*, 2004
- Green & Blackwell, "Cognitive Dimensions of Information Artefacts: a tutorial", 1998
- Bloch, "How to Design a Good API and Why it Matters", 2006