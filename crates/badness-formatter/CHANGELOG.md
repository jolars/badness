# Changelog

## [0.8.3](https://github.com/jolars/badness/compare/badness-formatter-v0.8.2...badness-formatter-v0.8.3) (2026-09-03)

### Bug Fixes
- **formatter:** stabilize long optionals ([`04d8ecf`](https://github.com/jolars/badness/commit/04d8ecfc280b9071b759724dcfa7347b75b20219))
- **formatter:** format empheq as math ([`e7a2140`](https://github.com/jolars/badness/commit/e7a21409a0164e27003b95521fcf19add3d719ee)), fixes [#172](https://github.com/jolars/badness/issues/172)
- **formatter:** explode column specs ([`583a98d`](https://github.com/jolars/badness/commit/583a98dd6ac96b6b1159132580c4eb33da80c345))

### Dependencies
- updated crates/badness-parser to v0.9.0

## [0.8.2](https://github.com/jolars/badness/compare/badness-formatter-v0.8.1...badness-formatter-v0.8.2) (2026-08-31)

### Bug Fixes
- **formatter:** keep dtx labels with headings ([`04d2073`](https://github.com/jolars/badness/commit/04d20736e8515e6cf17249e821eb8875cc64a548)), fixes [#166](https://github.com/jolars/badness/issues/166)
- **formatter:** attach citations to sentences ([`57e9bb8`](https://github.com/jolars/badness/commit/57e9bb8c25ff4315de9f4d978c930c7b41c21c15)), fixes [#163](https://github.com/jolars/badness/issues/163)
- **formatter:** indent environment header arguments ([`1431943`](https://github.com/jolars/badness/commit/14319433e12932a40fba8eb93b1813cd9de2ff97))
- **formatter:** normalize commented env args ([`6154eb9`](https://github.com/jolars/badness/commit/6154eb9a0bbf7e8d961ca66bc7863b9e1779654f))

### Dependencies
- updated crates/badness-parser to v0.8.1

## [0.8.1](https://github.com/jolars/badness/compare/badness-formatter-v0.8.0...badness-formatter-v0.8.1) (2026-08-28)

### Bug Fixes
- **formatter:** drop root block indentation ([`aad72e8`](https://github.com/jolars/badness/commit/aad72e87c593241beef4169ea9e06fc543815df1)), fixes [#162](https://github.com/jolars/badness/issues/162)
- **formatter:** align macro statement prefixes ([`4e7efa5`](https://github.com/jolars/badness/commit/4e7efa558386c94be8ba5da640b01a1d8dd35866)), fixes [#158](https://github.com/jolars/badness/issues/158)
- **formatter:** prioritize math breakpoints ([`9f85294`](https://github.com/jolars/badness/commit/9f852940dc914f6a589c1dd484cc78ba3fe0f47c))
- **formatter:** preserve postfix limit signs ([`658ef51`](https://github.com/jolars/badness/commit/658ef51b417d6af26e78d0ec839b050186998bfc))

### Dependencies
- updated crates/badness-parser to v0.8.0

## [0.8.0](https://github.com/jolars/badness/compare/badness-formatter-v0.7.0...badness-formatter-v0.8.0) (2026-08-27)

### Features
- **formatter:** close display math lines ([`7394f25`](https://github.com/jolars/badness/commit/7394f2547d34c22f1d005c7832273200ddae4d80))
- **formatter:** glue inline command arguments ([`f1093ff`](https://github.com/jolars/badness/commit/f1093ff47df04a3921f3608408e8f4b74de14a2d))
- **formatter:** split environment keyvals ([`3ea1412`](https://github.com/jolars/badness/commit/3ea14126ad154a33deab073d1142524370a29174))

### Bug Fixes
- **formatter:** preserve BibTeX lint directives ([`e0567a9`](https://github.com/jolars/badness/commit/e0567a9b2034a68737d0cb5b54f25a1c581bf8cc)), fixes [#159](https://github.com/jolars/badness/issues/159)
- **formatter:** align commented statements ([`440eca0`](https://github.com/jolars/badness/commit/440eca0d1f8f9ebe955e70a0e3d56e9be99b9b01)), fixes [#158](https://github.com/jolars/badness/issues/158)
- **formatter:** split commented begin tails ([`5d30318`](https://github.com/jolars/badness/commit/5d303186c912fb778ac09c7bbf56c612f50d508f))
- **formatter:** tighten keyval commas ([`d9a3453`](https://github.com/jolars/badness/commit/d9a3453f54dfaa3e8ef943a6ea8a7b25d469bec5))
- **formatter:** preserve authored line-break rows ([`4886414`](https://github.com/jolars/badness/commit/488641446488f0b9360e16890cec8ab5367c606f))
- **formatter:** keep labels with headings ([`7ba5fa5`](https://github.com/jolars/badness/commit/7ba5fa57be034a78598c248dbfa630b8c3852046))
- **formatter:** match omitted environment slots ([`c00340f`](https://github.com/jolars/badness/commit/c00340f45f3b0c55bf48becc2365e77cf2ea464f))
- **formatter:** honor control-word boundaries ([`4ee6fc6`](https://github.com/jolars/badness/commit/4ee6fc6d89bf1314f8a319fd8805835fd37a92c5))
- **formatter:** recognize unary math signs ([`6afa14f`](https://github.com/jolars/badness/commit/6afa14fa237327e9de92fbc0cbf1be3e4d8c6acc))

### Performance Improvements
- **formatter:** cache group break state ([`4a37963`](https://github.com/jolars/badness/commit/4a37963a7fefc8027997757ab4d7d24d150bf509))

### Dependencies
- updated crates/badness-parser to v0.7.0

## [0.7.0](https://github.com/jolars/badness/compare/badness-formatter-v0.6.0...badness-formatter-v0.7.0) (2026-08-26)

### Features
- **formatter:** configure item indentation ([`b869f10`](https://github.com/jolars/badness/commit/b869f10f482a53b0da061db8cf433c818f526889)), closes [#150](https://github.com/jolars/badness/issues/150)
- **formatter:** indent commented begin arguments ([`1da8350`](https://github.com/jolars/badness/commit/1da83509b392e0ca6df7ddf3a1f33c52851ba0f8))

### Bug Fixes
- **formatter:** close environment lines ([`c72d71d`](https://github.com/jolars/badness/commit/c72d71d5169f6681388c36fc944f8f306f930891))
- **formatter:** treat gathered as math ([`b31668c`](https://github.com/jolars/badness/commit/b31668c5cd9ce580ccc5d03514c94db62569fcb9))
- **formatter:** preserve math tail context ([`6dcb8ed`](https://github.com/jolars/badness/commit/6dcb8ed58b12f9ce85cedd686614dcc58b348229))
- **formatter:** align nested math environments ([`eeebbbf`](https://github.com/jolars/badness/commit/eeebbbf640c3d8c78dbfb4e0cd2c422fd7be6855))
- support Rust 1.94 ([`a7afd67`](https://github.com/jolars/badness/commit/a7afd671f94f11b0fda1931dc9eef7ba98b67679))

### Dependencies
- updated crates/badness-parser to v0.6.0

## [0.6.0](https://github.com/jolars/badness/compare/badness-formatter-v0.5.0...badness-formatter-v0.6.0) (2026-08-25)

### Features
- **formatter:** separate section headings ([`a75a46e`](https://github.com/jolars/badness/commit/a75a46efb9e5affa891f0ca433a44409507b5abf))
- **formatter:** align row terminators ([`7277610`](https://github.com/jolars/badness/commit/7277610c009de809144ae5fdfbd8b542a1e4823b))
- **lint:** detect extra alignment tabs ([`1129686`](https://github.com/jolars/badness/commit/11296869acb904d5d00457e74421122bd503feb6))

### Bug Fixes
- **formatter:** keep scripted colon relations tight ([`77833dc`](https://github.com/jolars/badness/commit/77833dc6b80ec1e4c9f4c209d735388145ff7a90))
- **formatter:** preserve colon relations ([`dbb64a2`](https://github.com/jolars/badness/commit/dbb64a29d381ce16f9ede755cdbb9b3c6a2d8af1))
- **lsp:** scope range formatting to selection ([`4439eeb`](https://github.com/jolars/badness/commit/4439eeb417c72403f8df081256ba20988c81b459)), fixes [#149](https://github.com/jolars/badness/issues/149)
- **formatter:** wrap long citation lists ([`8dbca9e`](https://github.com/jolars/badness/commit/8dbca9e4800b816424eee26c71fa747d475b3c67))

### Dependencies
- updated crates/badness-parser to v0.5.0

## [0.5.0](https://github.com/jolars/badness/compare/badness-formatter-v0.4.0...badness-formatter-v0.5.0) (2026-08-24)

### Features
- **formatter:** glue item overlays ([`e7b3efe`](https://github.com/jolars/badness/commit/e7b3efe42595e9c8f060b4700c79579981d90fc5))
- **formatter:** unify math spacing ([`170da11`](https://github.com/jolars/badness/commit/170da11a42e15a730e8d5606ea2f4e7f87297185))
- add semantic math atom classification ([`3889311`](https://github.com/jolars/badness/commit/388931180fec563d358aef9d2d20f8d6993ddaf1))
- **parser:** add argument domains ([`f7f4b01`](https://github.com/jolars/badness/commit/f7f4b011661f0106028c5e75896096b9b310f4d4))

### Bug Fixes
- **parser:** consume CRLF control symbols atomically ([`20dd182`](https://github.com/jolars/badness/commit/20dd18273e2aec5862abbb5d2d2f945b74467968))
- **formatter:** stabilize slash spacing ([`8f2e7ce`](https://github.com/jolars/badness/commit/8f2e7cee1140eb2e39a24211c963a70d50a34172)), fixes [#143](https://github.com/jolars/badness/issues/143)
- **formatter:** preserve trailing control newline ([`2a7977b`](https://github.com/jolars/badness/commit/2a7977b9083a3ff653c87c95768509cec27321f6)), fixes [#141](https://github.com/jolars/badness/issues/141)
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
