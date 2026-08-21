# Changelog

## [0.5.0](https://github.com/jolars/badness/compare/badness-formatter-v0.4.0...badness-formatter-v0.5.0) (2026-08-21)

### Features
- **formatter:** glue item overlays ([`e7b3efe`](https://github.com/jolars/badness/commit/e7b3efe42595e9c8f060b4700c79579981d90fc5))
- **formatter:** unify math spacing ([`170da11`](https://github.com/jolars/badness/commit/170da11a42e15a730e8d5606ea2f4e7f87297185))
- add semantic math atom classification ([`3889311`](https://github.com/jolars/badness/commit/388931180fec563d358aef9d2d20f8d6993ddaf1))
- **parser:** add argument domains ([`f7f4b01`](https://github.com/jolars/badness/commit/f7f4b011661f0106028c5e75896096b9b310f4d4))

### Bug Fixes
- **parser:** parse href URLs verbatim ([`03ad84f`](https://github.com/jolars/badness/commit/03ad84f6d78307d155a543b52047e6fc33591d51))
- **formatter:** preserve glued forced blocks ([`4938f6c`](https://github.com/jolars/badness/commit/4938f6c1a93cedf8ca6a5bfac592b839d5a28226))
- **formatter:** align virtual dtx tables ([`e0a2cde`](https://github.com/jolars/badness/commit/e0a2cde90da208f5f7b8cad42987ee0ec8bd8099))
- **formatter:** bound relation alignment ([`a560698`](https://github.com/jolars/badness/commit/a5606989918276da0d9579e7df9abddf2d4180e7))
- **formatter:** preserve math line breaks ([`a1ad967`](https://github.com/jolars/badness/commit/a1ad9676f609c6664371e18365833280cda20018))

### Dependencies
- updated crates/badness-parser to v0.4.0

## [0.4.0](https://github.com/jolars/badness/compare/badness-formatter-v0.3.0...badness-formatter-v0.4.0) (2026-08-20)

### Breaking changes
- **parser:** arity-directed expl3 attachment (#119) ([`5f2f9d8`](https://github.com/jolars/badness/commit/5f2f9d8d7d1abd93b054616e34ed07aa121da662))

### Features
- **formatter:** format DTX doc environments ([`ac98c95`](https://github.com/jolars/badness/commit/ac98c9564f4e7a4082cc7fe2be0fa3a3c3e678a3)), fixes [#127](https://github.com/jolars/badness/issues/127)
- **parser:** arity-directed expl3 attachment (#119) ([`5f2f9d8`](https://github.com/jolars/badness/commit/5f2f9d8d7d1abd93b054616e34ed07aa121da662))
- **formatter:** wrap picture statements at TikZ unit boundaries ([`5079f96`](https://github.com/jolars/badness/commit/5079f9607c5c8fcc9d021ba5b270347f7c6af524))
- **formatter:** hang statement continuations in picture bodies ([`5266aba`](https://github.com/jolars/badness/commit/5266aba65c4f0caa2cf5deebe8925ef70c9b9689))
- **parser:** wrap picture-body statements in STATEMENT nodes ([`abb16f4`](https://github.com/jolars/badness/commit/abb16f4948aea7e275cdba622ab4571701cd7682))

### Bug Fixes
- **formatter:** preserve verbatim arguments ([`7d39e61`](https://github.com/jolars/badness/commit/7d39e61cd3854c41c43ef7c4cee938c968259a2b)), fixes [#134](https://github.com/jolars/badness/issues/134)
- **formatter:** preserve TeX line semantics ([`f16188b`](https://github.com/jolars/badness/commit/f16188b5adb7b0132a58dbc05fcd435456843c3b)), fixes [#132](https://github.com/jolars/badness/issues/132)
- **formatter:** handle mixed dtx doc regions ([`6edfd81`](https://github.com/jolars/badness/commit/6edfd81f713baa6118838e23d79edbce0653fa76)), fixes [#126](https://github.com/jolars/badness/issues/126)
- preserve dtx documentation math ([`de0c54c`](https://github.com/jolars/badness/commit/de0c54cbedb2ae7cea6ab55204278fdd5bf2a49f)), fixes [#138](https://github.com/jolars/badness/issues/138)
- **formatter:** preserve DTX margin semantics ([`5dc60ec`](https://github.com/jolars/badness/commit/5dc60ecf24ab42152a2ae052c9f4fd615a966c5f)), fixes [#125](https://github.com/jolars/badness/issues/125)
- **formatter:** stabilize DTX prose atoms ([`35164ab`](https://github.com/jolars/badness/commit/35164ab7e008df1ead9c54886d4cf3bc74e35fbf)), fixes [#128](https://github.com/jolars/badness/issues/128)
- **formatter:** preserve guarded dtx paragraphs ([`0373420`](https://github.com/jolars/badness/commit/03734200431df441c48c706444633df287e26542)), fixes [#123](https://github.com/jolars/badness/issues/123)
- **formatter:** preserve guarded dtx commands ([`3d5d7ea`](https://github.com/jolars/badness/commit/3d5d7eaf2eac254bcff86b91bfe6103e36b548fd)), ref [#122](https://github.com/jolars/badness/issues/122)
- fix formatter keyval separator idempotency ([`3d6ffa7`](https://github.com/jolars/badness/commit/3d6ffa7398053783709456c6f467ca3ec2873a81)), closes [#121](https://github.com/jolars/badness/issues/121)
- **formatter:** retain math environment begin tails ([`2af7cb1`](https://github.com/jolars/badness/commit/2af7cb1aa5b1f6524a7274ed5eacdbac89197e94))

### Dependencies
- updated crates/badness-parser to v0.3.0

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
