<!-- Keep it short. For larger changes, please open an issue first to discuss
     the approach. -->

**What and why**

<!-- What does this change and what problem does it solve? Link related issues,
     e.g. `Closes #123`. -->

**Approach**

<!-- Anything reviewers should know: design choices, trade-offs, alternatives. -->

**Checklist**

- [ ] Added or updated tests (test-first; a bug fix has a failing fixture that now passes)
- [ ] New parser features have corpus + snapshot tests and a losslessness assertion
- [ ] `cargo test` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` is clean
- [ ] `cargo fmt -- --check` is clean
- [ ] Reviewed changed snapshots (`cargo insta review`), if any
- [ ] Commits follow Conventional Commits
