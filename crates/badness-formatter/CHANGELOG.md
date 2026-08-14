# Changelog

## [0.3.0](https://github.com/jolars/badness/compare/badness-formatter-v0.2.0...badness-formatter-v0.3.0) (2026-08-14)

### Features
- **config:** declare environments in `badness.toml` (#115) ([`a80b5af`](https://github.com/jolars/badness/commit/a80b5af3a42d8587cd323eb7f3e7bbdb4e20da5b))
- **formatter:** lay out picture bodies as statements ([`b437091`](https://github.com/jolars/badness/commit/b4370913f7bda378a41ca5b10c4ab01eb1cee35c)), closes [#114](https://github.com/jolars/badness/issues/114)
- **linter:** add `% badness-lint` suppression directives ([`c03114d`](https://github.com/jolars/badness/commit/c03114d711e15157c22d7fc98c928e13da5f1285)), refs [#114](https://github.com/jolars/badness/issues/114)
- **formatter:** add suppression comment directives ([`1810cde`](https://github.com/jolars/badness/commit/1810cdedaf0e8290a0ab04e28ba0f4360f4f282e)), refs [#114](https://github.com/jolars/badness/issues/114)
- **formatter:** segment a mandatory keyval group ([`d79ec73`](https://github.com/jolars/badness/commit/d79ec73a7cef2317880cd33b2eac98725cd05f8a))
- **formatter:** width-driven layout for opaque brace groups ([`03022a8`](https://github.com/jolars/badness/commit/03022a87fd312e08411c71e3ed6e65e5e30ec669))
- **cli:** add the strict trivia-invariance check ([`3796d7a`](https://github.com/jolars/badness/commit/3796d7a876a464ffa1f7284678b73e6683bde732))
- **parser:** pair user-defined environment delimiters ([`2bbff60`](https://github.com/jolars/badness/commit/2bbff600db873eb5c61008972924b318b7f01d4e)), closes [#109](https://github.com/jolars/badness/issues/109)
- **formatter:** lay conditionals out all-or-nothing ([`ed84bfe`](https://github.com/jolars/badness/commit/ed84bfef3441f467d40737bd12255dfcefbd6b71))
- **bib:** parse and preserve `%` comments ([`e005cc9`](https://github.com/jolars/badness/commit/e005cc96242ca01d886e18c2e32ccd027f8471c3))
- **formatter:** explode sibling-attached expl3 branches ([`d3fc51a`](https://github.com/jolars/badness/commit/d3fc51a9c5f7e8274e3cca8db885be1cc4831d77))
- **formatter:** expand optional arguments to the width ([`4c28ba4`](https://github.com/jolars/badness/commit/4c28ba4898ee417516acbc168b62388c3e3ba6d5))
- **formatter:** add optional serde and schema features ([`80726c7`](https://github.com/jolars/badness/commit/80726c75438afedf4f4d272ddae41707c178c364))
- **formatter:** reflow doc-margined out-of-region expl3 runs ([`aa9445a`](https://github.com/jolars/badness/commit/aa9445a015b47f3cee85980ca7aa259e05b29562))
- **formatter:** reflow dtx prose around margined blocks ([`4f118a2`](https://github.com/jolars/badness/commit/4f118a2a946cc2a6ef44afd4129f082aa2c68c34))

### Bug Fixes
- **formatter:** break a keyval group's glued opener ([`507a982`](https://github.com/jolars/badness/commit/507a982856b1c1b706328b04490cdc63cc3dfb32))
- **formatter:** body a `\begin` tail past the declared arity ([`bd7028e`](https://github.com/jolars/badness/commit/bd7028e3d229b89eb7f38b958b0840c0b7b48448))
- **formatter:** guard a prose argument's edge comments ([`8976815`](https://github.com/jolars/badness/commit/8976815e40136b5c1bcc31e2dac9f20096f27075))
- **formatter:** make optional fallbacks deterministic ([`9d2095c`](https://github.com/jolars/badness/commit/9d2095c18b24d4f1e5a1c0cb17979218f6e564d6))
- **formatter:** break around curated block-level commands ([`09b8d4f`](https://github.com/jolars/badness/commit/09b8d4f182f48a07fb77dd770e31f2b0835e213f))
- **parser:** harden environment-alias pairing ([`84f11a2`](https://github.com/jolars/badness/commit/84f11a2a3fbc931fbe19cf87e3d9ba125e35323d))
- **formatter:** correct what `lower_conditional` assumes ([`902dbd9`](https://github.com/jolars/badness/commit/902dbd981991fdc5ce74d570e365c92d37e323b8))
- **formatter:** break around sectioning commands ([`f4be809`](https://github.com/jolars/badness/commit/f4be80984f304db5c989a48d41d7c24e7c601715))
- **formatter:** stop deleting `]` inside prose arguments ([`7d2799f`](https://github.com/jolars/badness/commit/7d2799fd988f820ed7b42e66766d9cfb53f66fae))

### Performance Improvements
- **formatter:** gate the doc-margin scans on `cx.is_dtx` ([`4e7babf`](https://github.com/jolars/badness/commit/4e7babfe3b2a828e86f6102fbbffff353e956838))

### Dependencies
- updated crates/badness-parser to v0.2.0

## [0.2.0](https://github.com/jolars/badness/compare/badness-formatter-v0.1.0...badness-formatter-v0.2.0) (2026-08-07)

### Features
- **formatter:** default every file kind to reflow ([`ba9f2f9`](https://github.com/jolars/badness/commit/ba9f2f9dde4d8f50e073f53b6fd0b643787c0539))
- **formatter:** add a line-ending style ([`373a16c`](https://github.com/jolars/badness/commit/373a16c5012b53d4d8f500e19f396e0c66728aa2))

### Bug Fixes
- **formatter:** hug detonating atoms in fallback fills ([`db63ddb`](https://github.com/jolars/badness/commit/db63ddbf49cf58bc5534106894131e9a2686bec1))
- **formatter:** accept a relation as an expl3 `N` slot ([`4a3d92b`](https://github.com/jolars/badness/commit/4a3d92b908081b34c0899f471efabe8d573e3c3e)), closes [#106](https://github.com/jolars/badness/issues/106)
- **formatter:** gate the expl3 forced-break dispatch in fallback lines ([`7437f69`](https://github.com/jolars/badness/commit/7437f692b549ef64f7450e4e63450ba09943181b))
- **formatter:** keep fitting math segments flat ([`903ec3e`](https://github.com/jolars/badness/commit/903ec3e240023ea1f68af3b8a1a78d4151b3e97d))

### Dependencies
- updated crates/badness-parser to v0.1.1
