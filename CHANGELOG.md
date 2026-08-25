# Changelog

## [0.19.0](https://github.com/jolars/badness/compare/v0.18.0...v0.19.0) (2026-08-25)

### Features
- **linter:** check labels before list items ([`779e9cc`](https://github.com/jolars/badness/commit/779e9cc8492c252968c1dfdecc6a7b927a268b71))
- **lint:** detect invalid macrocode frames ([`9be2612`](https://github.com/jolars/badness/commit/9be26120c82d3f60c9277781b5e4c17c9bbe23a4))
- **ci:** scan arXiv source projects ([`c5e256b`](https://github.com/jolars/badness/commit/c5e256bc972ab2e340825eb68eccd20946ee3894)), ref [#152](https://github.com/jolars/badness/issues/152)
- **formatter:** separate section headings ([`a75a46e`](https://github.com/jolars/badness/commit/a75a46efb9e5affa891f0ca433a44409507b5abf))
- collect labels from environment options ([`df6f1f3`](https://github.com/jolars/badness/commit/df6f1f3eb336f906c0388794c7863f38a1714304))
- add command declarations ([`c9effb8`](https://github.com/jolars/badness/commit/c9effb8ffa84d52c3043a4e25d5dfe9921194d64))
- **lsp:** add table column refactor ([`def2998`](https://github.com/jolars/badness/commit/def299846b63117a15006db3e27a5889dc46afb6))
- **formatter:** align row terminators ([`7277610`](https://github.com/jolars/badness/commit/7277610c009de809144ae5fdfbd8b542a1e4823b))
- **lint:** detect extra alignment tabs ([`1129686`](https://github.com/jolars/badness/commit/11296869acb904d5d00457e74421122bd503feb6))

### Bug Fixes
- **formatter:** keep scripted colon relations tight ([`77833dc`](https://github.com/jolars/badness/commit/77833dc6b80ec1e4c9f4c209d735388145ff7a90))
- **formatter:** preserve colon relations ([`dbb64a2`](https://github.com/jolars/badness/commit/dbb64a29d381ce16f9ede755cdbb9b3c6a2d8af1))
- **lsp:** refresh cached config ([`27f22d0`](https://github.com/jolars/badness/commit/27f22d0261fcae03aedbae59b78b5cd4392e2960))
- **parser:** close citation command set ([`e10aa54`](https://github.com/jolars/badness/commit/e10aa540cc1cb281bba6308e54a4847b6e4045a1))
- **linter:** handle nested subfigure labels ([`bb0fed5`](https://github.com/jolars/badness/commit/bb0fed5552c64661b0d35b7e96afdf8653dbd66b))
- **lsp:** scope range formatting to selection ([`4439eeb`](https://github.com/jolars/badness/commit/4439eeb417c72403f8df081256ba20988c81b459)), fixes [#149](https://github.com/jolars/badness/issues/149)
- **formatter:** wrap long citation lists ([`8dbca9e`](https://github.com/jolars/badness/commit/8dbca9e4800b816424eee26c71fa747d475b3c67))
- honor curated ref/cite families in declarations ([`a3aa4ba`](https://github.com/jolars/badness/commit/a3aa4bab04ebd14960ea8cc18efc67d125513b86))

### Performance Improvements
- firewall declarations by parse and semantic tier ([`ef61239`](https://github.com/jolars/badness/commit/ef61239fa203af274d8de25479dd2fff72a2e49f))

### Dependencies
- updated crates/badness-formatter to v0.6.0
- updated crates/badness-parser to v0.5.0

## [0.18.0](https://github.com/jolars/badness/compare/v0.17.0...v0.18.0) (2026-08-24)

### Features
- add LSP memory benchmark ([`9a975d7`](https://github.com/jolars/badness/commit/9a975d7dcee69bc143a90ecff041b36d5de356c2))
- **formatter:** glue item overlays ([`e7b3efe`](https://github.com/jolars/badness/commit/e7b3efe42595e9c8f060b4700c79579981d90fc5))
- **parser:** reparse math fragments ([`4638eb5`](https://github.com/jolars/badness/commit/4638eb5a644019235ef976420e58f47834354fc2))
- **formatter:** unify math spacing ([`170da11`](https://github.com/jolars/badness/commit/170da11a42e15a730e8d5606ea2f4e7f87297185))
- add semantic math atom classification ([`3889311`](https://github.com/jolars/badness/commit/388931180fec563d358aef9d2d20f8d6993ddaf1))
- **parser:** add argument domains ([`f7f4b01`](https://github.com/jolars/badness/commit/f7f4b011661f0106028c5e75896096b9b310f4d4))

### Bug Fixes
- **parser:** consume CRLF control symbols atomically ([`20dd182`](https://github.com/jolars/badness/commit/20dd18273e2aec5862abbb5d2d2f945b74467968))
- **parser:** parse alignment char constants ([`78f3cd9`](https://github.com/jolars/badness/commit/78f3cd962b17379141efda3067495df8723f1b67))
- **parser:** guard expl3 mode boundaries ([`08c0387`](https://github.com/jolars/badness/commit/08c0387ec0cad42ad04782d2175f5e61ef294fa3))
- **formatter:** preserve trailing control newline ([`2a7977b`](https://github.com/jolars/badness/commit/2a7977b9083a3ff653c87c95768509cec27321f6)), fixes [#141](https://github.com/jolars/badness/issues/141)
- **lsp:** replace project snapshot generations ([`a1194cb`](https://github.com/jolars/badness/commit/a1194cbcd54e33964e94081541fca1ab5fbc6141))
- **lsp:** gate query logging ([`f212680`](https://github.com/jolars/badness/commit/f2126807582f78e8ed32b2d7b87ad9112cd0ec65))
- **parser:** tighten catcode signal ([`834c7b6`](https://github.com/jolars/badness/commit/834c7b65b6ab319430e1ed86fdb62582f3b61812))
- **parser:** pass plain braces through optionals ([`82b30f1`](https://github.com/jolars/badness/commit/82b30f17190f935432c3df59fc6428ed33003562))
- **parser:** parse href URLs verbatim ([`03ad84f`](https://github.com/jolars/badness/commit/03ad84f6d78307d155a543b52047e6fc33591d51))
- **formatter:** preserve glued forced blocks ([`4938f6c`](https://github.com/jolars/badness/commit/4938f6c1a93cedf8ca6a5bfac592b839d5a28226))
- **formatter:** align virtual dtx tables ([`e0a2cde`](https://github.com/jolars/badness/commit/e0a2cde90da208f5f7b8cad42987ee0ec8bd8099))
- **parser:** preserve argument mode semantics ([`63aefa8`](https://github.com/jolars/badness/commit/63aefa8e626f9e7e1012ca02ab0370440441c5b2))
- **formatter:** bound relation alignment ([`a560698`](https://github.com/jolars/badness/commit/a5606989918276da0d9579e7df9abddf2d4180e7))
- **formatter:** preserve math line breaks ([`a1ad967`](https://github.com/jolars/badness/commit/a1ad9676f609c6664371e18365833280cda20018))
- **parser:** honor TeX script atom boundaries ([`e6131b6`](https://github.com/jolars/badness/commit/e6131b69e1df9f3fed25af772fce803655b3e88d))

### Dependencies
- updated crates/badness-formatter to v0.5.0
- updated crates/badness-parser to v0.4.0

## [0.17.0](https://github.com/jolars/badness/compare/v0.16.0...v0.17.0) (2026-08-20)

### Breaking changes
- **parser:** pair one-sided environment aliases ([`d757cdc`](https://github.com/jolars/badness/commit/d757cdca2dd65809ebd7f08f0e3e288e5036c45c)), closes [#117](https://github.com/jolars/badness/issues/117)
- **parser:** arity-directed expl3 attachment (#119) ([`5f2f9d8`](https://github.com/jolars/badness/commit/5f2f9d8d7d1abd93b054616e34ed07aa121da662))

### Features
- **formatter:** format DTX doc environments ([`ac98c95`](https://github.com/jolars/badness/commit/ac98c9564f4e7a4082cc7fe2be0fa3a3c3e678a3)), fixes [#127](https://github.com/jolars/badness/issues/127)
- **parser:** intra-file incremental reparse (#130) ([`393e0c3`](https://github.com/jolars/badness/commit/393e0c3e85b10226e58afb7b37f78cfe535dd9fd))
- **bench:** time the keystroke pipeline ([`b1b2b0e`](https://github.com/jolars/badness/commit/b1b2b0e1edf3d8952248e25c41c113047bcd7a97))
- **parser:** pair one-sided environment aliases ([`d757cdc`](https://github.com/jolars/badness/commit/d757cdca2dd65809ebd7f08f0e3e288e5036c45c)), closes [#117](https://github.com/jolars/badness/issues/117)
- **parser:** arity-directed expl3 attachment (#119) ([`5f2f9d8`](https://github.com/jolars/badness/commit/5f2f9d8d7d1abd93b054616e34ed07aa121da662))
- **formatter:** wrap picture statements at TikZ unit boundaries ([`5079f96`](https://github.com/jolars/badness/commit/5079f9607c5c8fcc9d021ba5b270347f7c6af524))
- **formatter:** hang statement continuations in picture bodies ([`5266aba`](https://github.com/jolars/badness/commit/5266aba65c4f0caa2cf5deebe8925ef70c9b9689))

### Bug Fixes
- **parser:** isolate command definition names ([`8d6e274`](https://github.com/jolars/badness/commit/8d6e274957da4781668e25d9f2cd8261ee87cd34)), fixes [#133](https://github.com/jolars/badness/issues/133)
- **formatter:** preserve TeX line semantics ([`f16188b`](https://github.com/jolars/badness/commit/f16188b5adb7b0132a58dbc05fcd435456843c3b)), fixes [#132](https://github.com/jolars/badness/issues/132)
- **formatter:** handle mixed dtx doc regions ([`6edfd81`](https://github.com/jolars/badness/commit/6edfd81f713baa6118838e23d79edbce0653fa76)), fixes [#126](https://github.com/jolars/badness/issues/126)
- preserve dtx documentation math ([`de0c54c`](https://github.com/jolars/badness/commit/de0c54cbedb2ae7cea6ab55204278fdd5bf2a49f)), fixes [#138](https://github.com/jolars/badness/issues/138)
- **formatter:** preserve guarded dtx paragraphs ([`0373420`](https://github.com/jolars/badness/commit/03734200431df441c48c706444633df287e26542)), fixes [#123](https://github.com/jolars/badness/issues/123)

### Performance Improvements
- **lsp:** share document text and line index ([`3d4a5f8`](https://github.com/jolars/badness/commit/3d4a5f89e89cb371e6648b7a6226eb633c97af8e))

### Dependencies
- updated crates/badness-formatter to v0.4.0
- updated crates/badness-parser to v0.3.0

## [0.16.0](https://github.com/jolars/badness/compare/v0.15.0...v0.16.0) (2026-08-14)

### Features
- **config:** declare environments in `badness.toml` (#115) ([`a80b5af`](https://github.com/jolars/badness/commit/a80b5af3a42d8587cd323eb7f3e7bbdb4e20da5b))
- **formatter:** lay out picture bodies as statements ([`b437091`](https://github.com/jolars/badness/commit/b4370913f7bda378a41ca5b10c4ab01eb1cee35c)), closes [#114](https://github.com/jolars/badness/issues/114)
- **linter:** add `% badness-lint` suppression directives ([`c03114d`](https://github.com/jolars/badness/commit/c03114d711e15157c22d7fc98c928e13da5f1285)), refs [#114](https://github.com/jolars/badness/issues/114)
- **formatter:** add suppression comment directives ([`1810cde`](https://github.com/jolars/badness/commit/1810cdedaf0e8290a0ab04e28ba0f4360f4f282e)), refs [#114](https://github.com/jolars/badness/issues/114)
- **linter:** add `blank-line-in-keyval` ([`0758ea4`](https://github.com/jolars/badness/commit/0758ea4a8b069b3b5a848ef9012651f7252f1daf))
- **formatter:** segment a mandatory keyval group ([`d79ec73`](https://github.com/jolars/badness/commit/d79ec73a7cef2317880cd33b2eac98725cd05f8a))
- **formatter:** width-driven layout for opaque brace groups ([`03022a8`](https://github.com/jolars/badness/commit/03022a87fd312e08411c71e3ed6e65e5e30ec669))
- **cli:** add the strict trivia-invariance check ([`3796d7a`](https://github.com/jolars/badness/commit/3796d7a876a464ffa1f7284678b73e6683bde732))
- **linter:** add `label-before-caption` rule ([`e8c5b7f`](https://github.com/jolars/badness/commit/e8c5b7fd233da1e7fe82c1a676369d16582beaaf))
- **linter:** colorize the pretty lint report ([`e4faf16`](https://github.com/jolars/badness/commit/e4faf1663d682a47e53247027adf36ce1f85fd4c))
- **parser:** pair user-defined environment delimiters ([`2bbff60`](https://github.com/jolars/badness/commit/2bbff600db873eb5c61008972924b318b7f01d4e)), closes [#109](https://github.com/jolars/badness/issues/109)
- **cli:** read stdin from `-`, not a bare terminal ([`5bb4788`](https://github.com/jolars/badness/commit/5bb47882af6742e28d2ed138d2f9bcf9dda92a15)), closes [#111](https://github.com/jolars/badness/issues/111)
- **formatter:** lay conditionals out all-or-nothing ([`ed84bfe`](https://github.com/jolars/badness/commit/ed84bfef3441f467d40737bd12255dfcefbd6b71))
- **parser:** gated `CONDITIONAL` node for `\if…\else…\or…\fi` ([`e0ca4ef`](https://github.com/jolars/badness/commit/e0ca4ef71e149ff715d59fb13afdbd973221d622))
- **bib:** parse and preserve `%` comments ([`e005cc9`](https://github.com/jolars/badness/commit/e005cc96242ca01d886e18c2e32ccd027f8471c3))
- **skill:** add formatter-fixture for construct coverage ([`a52095a`](https://github.com/jolars/badness/commit/a52095adece121207bfa8533d7a8680c498a3746))
- **lsp:** add inverse search over IPC ([`3b829eb`](https://github.com/jolars/badness/commit/3b829ebc368a74d5a678ccc93b533678b95bc60f))
- **lsp:** add `textDocument/forwardSearch` ([`8028a40`](https://github.com/jolars/badness/commit/8028a40bcd6b58d59f3cd3e3228f94386a0c8e2d))
- **formatter:** explode sibling-attached expl3 branches ([`d3fc51a`](https://github.com/jolars/badness/commit/d3fc51a9c5f7e8274e3cca8db885be1cc4831d77))
- **formatter:** expand optional arguments to the width ([`4c28ba4`](https://github.com/jolars/badness/commit/4c28ba4898ee417516acbc168b62388c3e3ba6d5))
- **formatter:** add optional serde and schema features ([`80726c7`](https://github.com/jolars/badness/commit/80726c75438afedf4f4d272ddae41707c178c364))
- **formatter:** reflow doc-margined out-of-region expl3 runs ([`aa9445a`](https://github.com/jolars/badness/commit/aa9445a015b47f3cee85980ca7aa259e05b29562))
- **formatter:** reflow dtx prose around margined blocks ([`4f118a2`](https://github.com/jolars/badness/commit/4f118a2a946cc2a6ef44afd4129f082aa2c68c34))
- **semantic:** add curated block-level command property ([`c54a5ff`](https://github.com/jolars/badness/commit/c54a5ffce69c459a39415a23ab210b5530fcdd37))

### Bug Fixes
- **formatter:** break a keyval group's glued opener ([`507a982`](https://github.com/jolars/badness/commit/507a982856b1c1b706328b04490cdc63cc3dfb32))
- **scripts:** keep pdflatex stdout off hyperref's `.out` ([`1e7c4c1`](https://github.com/jolars/badness/commit/1e7c4c1cc06bb6320a352aa87e7fc71da4bbb806))
- **formatter:** body a `\begin` tail past the declared arity ([`bd7028e`](https://github.com/jolars/badness/commit/bd7028e3d229b89eb7f38b958b0840c0b7b48448))
- **parser:** break every gate's run at a docstrip guard ([`d682e8f`](https://github.com/jolars/badness/commit/d682e8f6e648695a32f8a3502d3afb42f8b015ee))
- **formatter:** guard a prose argument's edge comments ([`8976815`](https://github.com/jolars/badness/commit/8976815e40136b5c1bcc31e2dac9f20096f27075))
- **formatter:** break around curated block-level commands ([`09b8d4f`](https://github.com/jolars/badness/commit/09b8d4f182f48a07fb77dd770e31f2b0835e213f))
- **linter:** pair straight quotes into one finding ([`e1ff0d1`](https://github.com/jolars/badness/commit/e1ff0d16de3ebdc7392a2a6ab7aefecd6885e14c))
- **parser:** harden environment-alias pairing ([`84f11a2`](https://github.com/jolars/badness/commit/84f11a2a3fbc931fbe19cf87e3d9ba125e35323d))
- **project:** link a subfile to its parent document ([`df3e66c`](https://github.com/jolars/badness/commit/df3e66c09857cef966beb27a76e569fbece7b0cd)), closes [#112](https://github.com/jolars/badness/issues/112)
- **lsp:** spell decoded URI paths with native separators ([`f199bd1`](https://github.com/jolars/badness/commit/f199bd1cb206f9f2480a20cdfe949529f1917cac))
- **formatter:** break around sectioning commands ([`f4be809`](https://github.com/jolars/badness/commit/f4be80984f304db5c989a48d41d7c24e7c601715))
- **formatter:** stop deleting `]` inside prose arguments ([`7d2799f`](https://github.com/jolars/badness/commit/7d2799fd988f820ed7b42e66766d9cfb53f66fae))
- **formatter:** make optional fallbacks deterministic ([`9d2095c`](https://github.com/jolars/badness/commit/9d2095c18b24d4f1e5a1c0cb17979218f6e564d6))
- **formatter:** correct what `lower_conditional` assumes ([`902dbd9`](https://github.com/jolars/badness/commit/902dbd981991fdc5ce74d570e365c92d37e323b8))

### Performance Improvements
- **cli:** use Histogram for the `--check` diff, measured on the corpora ([`29a678a`](https://github.com/jolars/badness/commit/29a678a9859b59c6245179f35618108d9299986b))
- **cli:** pick Patience over Histogram for the `--check` diff ([`595abb3`](https://github.com/jolars/badness/commit/595abb3b672d80badf7652f6a699af39597a3182))
- **cli:** diff `--check` with Histogram, write it buffered ([`f34abb8`](https://github.com/jolars/badness/commit/f34abb8f650ef3ece16777bc5b42eb83389064ae))
- **lint:** render pretty snippets from a line window ([`9dffc3d`](https://github.com/jolars/badness/commit/9dffc3d3ffa0c6607423e5351e07de3065a7c69c))
- **parser:** one batch driver for all nine shape gates (#113) ([`9e01ee5`](https://github.com/jolars/badness/commit/9e01ee557378bd11bde3f792be0b4de00ae75eca))
- **parser:** bound the environment-alias closer scan ([`ae83909`](https://github.com/jolars/badness/commit/ae83909940c2d1fba88f1e05aa05e461915f337a))
- **formatter:** gate the doc-margin scans on `cx.is_dtx` ([`4e7babf`](https://github.com/jolars/badness/commit/4e7babfe3b2a828e86f6102fbbffff353e956838))
- **parser:** answer `on_doc_margin_line` from a pre-scan ([`930380b`](https://github.com/jolars/badness/commit/930380b14671b5e0148af0fcfe99830e1f14a1da))

### Dependencies
- updated crates/badness-formatter to v0.3.0
- updated crates/badness-parser to v0.2.0

## [0.15.0](https://github.com/jolars/badness/compare/v0.14.0...v0.15.0) (2026-08-07)

### Features
- **formatter:** default every file kind to reflow ([`ba9f2f9`](https://github.com/jolars/badness/commit/ba9f2f9dde4d8f50e073f53b6fd0b643787c0539))
- **formatter:** add a line-ending style ([`373a16c`](https://github.com/jolars/badness/commit/373a16c5012b53d4d8f500e19f396e0c66728aa2))
- **wasm:** add `badness-wasm` playground shim crate ([`6486890`](https://github.com/jolars/badness/commit/648689051f65d7729a27821605ea1473e967be6b))

### Bug Fixes
- **formatter:** hug detonating atoms in fallback fills ([`db63ddb`](https://github.com/jolars/badness/commit/db63ddbf49cf58bc5534106894131e9a2686bec1))
- **formatter:** accept a relation as an expl3 `N` slot ([`4a3d92b`](https://github.com/jolars/badness/commit/4a3d92b908081b34c0899f471efabe8d573e3c3e)), closes [#106](https://github.com/jolars/badness/issues/106)
- **formatter:** gate the expl3 forced-break dispatch in fallback lines ([`7437f69`](https://github.com/jolars/badness/commit/7437f692b549ef64f7450e4e63450ba09943181b))
- **linter:** skip parameter-template keys in key scans ([`928aa4a`](https://github.com/jolars/badness/commit/928aa4ace51aa193430a1445f9d7d6afacff8f2e)), closes [#104](https://github.com/jolars/badness/issues/104)
- **formatter:** keep fitting math segments flat ([`903ec3e`](https://github.com/jolars/badness/commit/903ec3e240023ea1f68af3b8a1a78d4151b3e97d))

### Dependencies
- updated crates/badness-formatter to v0.2.0
- updated crates/badness-parser to v0.1.1

## [0.14.0](https://github.com/jolars/badness/compare/v0.13.0...v0.14.0) (2026-08-06)

### Features
- **cli:** diff changed files in `format --check` ([`eed1537`](https://github.com/jolars/badness/commit/eed1537d15fd467b09b89e0750a5decbb42d1cf7))
- **semantic:** curate filecontents and ltxdockit verbatim envs ([`d805548`](https://github.com/jolars/badness/commit/d8055483cc4716eb89327ea3c9a85f7f45cbe212)), closes [#98](https://github.com/jolars/badness/issues/98)
- **formatter:** respace flush expl3 argument braces ([`c44125c`](https://github.com/jolars/badness/commit/c44125cf40212a5ca518f0f9beda295de03d7069))
- **formatter:** add trivia-perturbation invariance oracle (#103) ([`f22d668`](https://github.com/jolars/badness/commit/f22d668c6c95d323b0cc3ecb110db811bd0f23ba))
- **packaging:** publish badness-bin to the AUR on release ([`82a64c2`](https://github.com/jolars/badness/commit/82a64c2284fe71a4155b481d22507901abffda1d))
- **lint:** add `--output json` machine-readable findings ([`92b04bd`](https://github.com/jolars/badness/commit/92b04bd48c35587e4157438e98960ab08171772b))
- **formatter:** explode expl3 conditional branches (R4) ([`bebdbde`](https://github.com/jolars/badness/commit/bebdbdecce2a493135c7da826259a322cfe097cb))
- **parser:** recognize package-defined verbatim envs ([`696f109`](https://github.com/jolars/badness/commit/696f109074c612fad2998ab194ece275c0b7713b))
- **build:** bundle man pages and completion in tarballs ([`8268c88`](https://github.com/jolars/badness/commit/8268c885ce8811dd33a533cd8f8085b998bdd1d2))

### Bug Fixes
- **formatter:** render expl3 conditionals all-or-nothing ([`c8b3aef`](https://github.com/jolars/badness/commit/c8b3aef1ac0b68671b5136233273fb15edcc384b))
- **formatter:** keep annotated expl3 branches on the exploded path ([`ac07506`](https://github.com/jolars/badness/commit/ac0750624e01506172d0a3192b0df35186f9928f)), refs [#101](https://github.com/jolars/badness/issues/101)
- **formatter:** drop expl3 sibling break coupling ([`836ed83`](https://github.com/jolars/badness/commit/836ed83b6b3ea4914dd4580457ea5ebd73299afb)), closes [#101](https://github.com/jolars/badness/issues/101)
- **packaging:** tolerate pre-0.14 release tarballs in PKGBUILD ([`750493b`](https://github.com/jolars/badness/commit/750493b97a142ff40b35ce250680c5d2d716e1aa))
- **installer:** detect musl/libc in installation script ([`eb195a1`](https://github.com/jolars/badness/commit/eb195a1904bbf0a7a3a8d2f83308a3539b748ffc))
- **formatter:** pin forced expl3 block body to break mode ([`349ccdd`](https://github.com/jolars/badness/commit/349ccdd26f1e269703bee96feaaec41a16d0fff2))
- **formatter:** stabilize trailing expl3 hang group ([`72a6d35`](https://github.com/jolars/badness/commit/72a6d3585bd3e4310cd26aeef347740c933c0979)), closes [#96](https://github.com/jolars/badness/issues/96)
- **npm:** fall back to musl when glibc build fails ([`bd1891e`](https://github.com/jolars/badness/commit/bd1891e0961bc36c0c6f088d5f61b33b0db0af47))
- **formatter:** stabilize trailing expl3 conditional ([`b5ed902`](https://github.com/jolars/badness/commit/b5ed902de89c7619c55d7a143784e21383500863)), refs [#96](https://github.com/jolars/badness/issues/96)
- **parser:** pair \left/\right inside macro code ([`20a59ef`](https://github.com/jolars/badness/commit/20a59eff44e3b6b4e4a0013291a99d7b2ae1221f)), closes [#95](https://github.com/jolars/badness/issues/95)
- **parser:** bound math bracket gate at dollar closer ([`703f5f5`](https://github.com/jolars/badness/commit/703f5f569acd01a92f57b8a5006c159401fe4249)), closes [#99](https://github.com/jolars/badness/issues/99)
- **formatter:** keep expl3 parameter runs tight ([`47d6277`](https://github.com/jolars/badness/commit/47d62775abcc2620329db9508a1f8a52bdeda55b)), exception [#1](https://github.com/jolars/badness/issues/1) and [#2](https://github.com/jolars/badness/issues/2)
- **formatter:** sticky-break fill for expl3 statements ([`107ecb0`](https://github.com/jolars/badness/commit/107ecb0e97525f5f10ed520599ca2a5e4fec882c)), closes [#94](https://github.com/jolars/badness/issues/94)
- **linter:** stop TikZ/pgf false positives ([`aca172f`](https://github.com/jolars/badness/commit/aca172f3bc782bf98dc6836a4f21c80711e38a6d))
- **linter:** drop "Part", gate hard-coded-reference item labels ([`274c124`](https://github.com/jolars/badness/commit/274c1243d94cf451bef6f03317ecf1c60eb61277))
- **linter:** gate space-before-command on trailing break ([`2f66cae`](https://github.com/jolars/badness/commit/2f66cae638337bc56f21ca7838e6884586720173))
- **linter:** skip hex constants, font maps in straight-quotes ([`2fb9ee3`](https://github.com/jolars/badness/commit/2fb9ee31f7c6709616526a2c1bafb03eec225afa))
- **linter:** skip redefined commands in deprecated/primitive ([`0490f16`](https://github.com/jolars/badness/commit/0490f16c3a708d6cdc0b24c93ceb82fb3e5358ad))
- **linter:** skip starred headings in sectioning-level-jump ([`befe30e`](https://github.com/jolars/badness/commit/befe30e44cf7e6d6528333a164dc56988365d612))
- **parser:** parse array and tikzcd bodies as math ([`9db4e8f`](https://github.com/jolars/badness/commit/9db4e8fc3f492303479ee572d4e50aba88f85622))

## [0.13.0](https://github.com/jolars/badness/compare/v0.12.0...v0.13.0) (2026-08-01)

### Features
- **semantic:** curate `codeexample` as verbatim env ([`4fb98f7`](https://github.com/jolars/badness/commit/4fb98f7d764030cc7d1af8d875eb35b914ad10d8))
- **lexer:** infer expl3 in toggle-less `.dtx` ([`caba767`](https://github.com/jolars/badness/commit/caba767610b66a266d0f5f35f2851c06fd94b5ac))
- **lsp:** link package docs via texdoc in hover ([`40b2665`](https://github.com/jolars/badness/commit/40b26651aaa52c9c3139689d364475e68dbacf31))
- **lsp:** filter cite completion by title and author ([`509cd4a`](https://github.com/jolars/badness/commit/509cd4a08ee30854c01f53be3a559fab1d26029c))
- **linter:** express and apply cross-file fixes ([`e9071f8`](https://github.com/jolars/badness/commit/e9071f83c485cc23f2dd9cdcbca080fd6cbfe9f7))
- **vscode:** add feature toggles for the LSP ([`bb3f13c`](https://github.com/jolars/badness/commit/bb3f13c7f5d0581b2f70ae16422de2946238d3cc)), closes [#86](https://github.com/jolars/badness/issues/86)
- **formatter:** align user environments on `&` ([`ee88953`](https://github.com/jolars/badness/commit/ee88953554b643460b8ff719dcb2ba8a4a275143)), closes [#84](https://github.com/jolars/badness/issues/84)
- **formatter:** hang expl3 attached brace arguments ([`c25c91b`](https://github.com/jolars/badness/commit/c25c91b97b7c57b9680f207fd5495da8e96fac7a))
- **formatter:** hang \item continuations under preserve ([`5edeea7`](https://github.com/jolars/badness/commit/5edeea7d32f39c1c0c46bccfa2345d57cd3efcdc)), closes [#82](https://github.com/jolars/badness/issues/82)
- **incremental:** mark SourceFile.path HIGH durability ([`bca096a`](https://github.com/jolars/badness/commit/bca096a8eb8dafbf94c0d22c79957fe02afc0064))
- **linter:** flag unclosed math delimiters as likely typos ([`4731c18`](https://github.com/jolars/badness/commit/4731c181f184934c110bfa3d316011d0f41f071b))

### Bug Fixes
- **parser:** parse `.code.tex` under package flavor ([`2694a86`](https://github.com/jolars/badness/commit/2694a8659b858c9e9ca26841a37efd5ee5421775))
- **linter:** withhold deprecated-command fix in reference position ([`668647a`](https://github.com/jolars/badness/commit/668647ab18e7a6034beb58c1caabec807395d717))
- **linter:** skip compound-logo swallowed space ([`0a546b7`](https://github.com/jolars/badness/commit/0a546b7b6c78d2fad4ffd4eed20244d826f6ec87))
- **linter:** ignore `\string`-prefixed package loads ([`e20504e`](https://github.com/jolars/badness/commit/e20504e341be9df4b5d6e91f9ccbed6c77ab26fe))
- **linter:** skip `\texttt` dashes and `\foreach` ranges ([`f605ee5`](https://github.com/jolars/badness/commit/f605ee50cb42f08844b867bd8b4ab5108f9b9593))
- **linter:** skip citation locators and env titles in hard-coded-reference ([`620bde5`](https://github.com/jolars/badness/commit/620bde59c654f00bb638dacc276e6a1828062de7))
- **linter:** skip xypic `@` DSL in `makeat-macro` ([`81e4c81`](https://github.com/jolars/badness/commit/81e4c81065c4c82e9da98f2d74c4e102dc24b395))
- **parser:** attach math optional across balanced `$...$` ([`b5c6c50`](https://github.com/jolars/badness/commit/b5c6c50372d7b75cc343aeb178e134677c4e2003))
- **formatter:** scope preserve spacing collapse to prose ([`0999610`](https://github.com/jolars/badness/commit/09996109d091686e46e22aa9a3010aaed893c982))
- **formatter:** normalize inner spacing under preserve ([`be8d5ba`](https://github.com/jolars/badness/commit/be8d5ba3b495eaa42d8e6862fdc91fadab447a69))
- **linter:** skip prose rules in Lua code and doc placeholders ([`bcd4f44`](https://github.com/jolars/badness/commit/bcd4f4432652043f644f212c29de6e98e2606186))

### Performance Improvements
- **line-index:** precompute wide-char table, own no text ([`d9c526f`](https://github.com/jolars/badness/commit/d9c526fc62eae6ed39c24fb11c4f21471d9443aa))

## [0.12.0](https://github.com/jolars/badness/compare/v0.11.0...v0.12.0) (2026-07-30)

### Features
- **formatter:** break leading `\label` onto its own line ([`18b17c5`](https://github.com/jolars/badness/commit/18b17c59a571ca72fca0e0b9a71f2bdeaee57f50))
- add stable-diff paragraph wrapping (#41) ([`5734632`](https://github.com/jolars/badness/commit/5734632d6aa68c39c0f6ceec8e84733b54c8be59))
- **format:** indent expl3 continuation groups one step ([`d53f064`](https://github.com/jolars/badness/commit/d53f064568ede8631df3f2f00fef4a05dba7c4d2))
- **config:** add `BADNESS_CONFIG` env var for config path ([`b8756e8`](https://github.com/jolars/badness/commit/b8756e8f6cb2d6e7be54df4e9e361b496dc5b875))
- **cli:** add hidden `debug format` check command ([`da5959f`](https://github.com/jolars/badness/commit/da5959f378b7beb6114c1059916c849525c5896b))
- **config:** add global user config fallback ([`41c4570`](https://github.com/jolars/badness/commit/41c4570ba80b8af63347850575289b98148162b0)), closes [#40](https://github.com/jolars/badness/issues/40)
- **formatter:** add `math-wrap` display-math break policy ([`dbba5eb`](https://github.com/jolars/badness/commit/dbba5eb9d1bb3f510d48d2802be7eb2a7bc4385e)), closes [#42](https://github.com/jolars/badness/issues/42)

### Bug Fixes
- **formatter:** pin inter-argument docstrip guards to column 0 ([`3556c79`](https://github.com/jolars/badness/commit/3556c79f5034b20ce559d3aa2432108ff5f5720a)), closes [#78](https://github.com/jolars/badness/issues/78)
- **formatter:** keep stable-wrap break mask aligned on Nil atoms ([`d4211b1`](https://github.com/jolars/badness/commit/d4211b11294da5ab664488af07bcb167394f9d3e))
- **parser:** shape-gate unclosed `\left` instead of erroring ([`29aa319`](https://github.com/jolars/badness/commit/29aa319b95e568508cd434d69e89cbbc395ab640)), closes [#77](https://github.com/jolars/badness/issues/77)
- **parser:** close the latex2e format-error buckets from the smoke test (#80) ([`bb9484e`](https://github.com/jolars/badness/commit/bb9484e0da36d3fe2ab27230c82e54df7799196a))
- **parser:** stop the lexer hiding braces, guards, and short-verb bars (#79) ([`a6e2119`](https://github.com/jolars/badness/commit/a6e2119b61ab955d890fd60f2ef8bc9072421137)), refs [#71](https://github.com/jolars/badness/issues/71)
- **parser:** stop environments escaping their brace group (#75) ([`7d82f35`](https://github.com/jolars/badness/commit/7d82f356cfea115e4c06b15fa1869eb38adb0139)), refs [#71](https://github.com/jolars/badness/issues/71)
- **parser:** scope the math gates' paragraph-break anchor to the body's own level (#74) ([`ef89b76`](https://github.com/jolars/badness/commit/ef89b76ea66ce057e9927b5ccb97a61190715128)), closes [#70](https://github.com/jolars/badness/issues/70)
- gate expl3 relayout to top-level toggles ([`81a1a92`](https://github.com/jolars/badness/commit/81a1a926534cb5a38c5ead0f1d21843b34540789)), closes [#69](https://github.com/jolars/badness/issues/69)
- **parser:** suppress braced verbatim on redefinition ([`513e963`](https://github.com/jolars/badness/commit/513e96394af01f1a0bd16f48a95882baaec180e2))
- **formatter:** preserve fully-guarded expl3 chunks ([`5d2e46b`](https://github.com/jolars/badness/commit/5d2e46ba0c31f3ed6e140e640fd9c24d447ac362)), closes [#72](https://github.com/jolars/badness/issues/72)
- **parser:** treat escaped backtick char constant as data ([`d7edf4b`](https://github.com/jolars/badness/commit/d7edf4bb302b3d14828106b13f647d2c58d698f8)), refs [#71](https://github.com/jolars/badness/issues/71)
- **format:** keep glued brace opener on its line ([`97e7abb`](https://github.com/jolars/badness/commit/97e7abb98e20206621c2009d8ba5c615186f0bc6))
- **format:** treat sign after opener as unary in math ([`94af6dd`](https://github.com/jolars/badness/commit/94af6ddf98b53e0bd9ddff4daa007cf1b10593fe))
- **format:** keep guarded expl3 code groups broken ([`417c480`](https://github.com/jolars/badness/commit/417c4805210e19dace9778f84ef883e0df4fafaa)), closes [#61](https://github.com/jolars/badness/issues/61)
- **format:** keep doc-margin math environments verbatim ([`d8e3864`](https://github.com/jolars/badness/commit/d8e38643132f429f84435e0296669d14dbb3bd26)), refs [#61](https://github.com/jolars/badness/issues/61)
- **format:** make trailing comments zero-width in expl3 code ([`db53fc8`](https://github.com/jolars/badness/commit/db53fc8c65299bc7ea90da0272f5ae21730548bc))
- **format:** keep doc-commented expl3 statements whole ([`fb61e15`](https://github.com/jolars/badness/commit/fb61e15f02bea005e92292e28f1c684289917341))
- **parser:** treat expl3 regions and v-arg names as macro data ([`e9deecf`](https://github.com/jolars/badness/commit/e9deecfa009de7ee4cc0e7a9a08f7922da1f8b34)), issue [#60](https://github.com/jolars/badness/issues/60)
- **parser:** add `^^A`, v-arg, and char-constant lexing ([`ff2b516`](https://github.com/jolars/badness/commit/ff2b516004ce3a6d7020a775f00f295149ce65d6))
- **parser:** gate `\[`/`\(` and isolate `\def`-family names ([`bb55149`](https://github.com/jolars/badness/commit/bb551495e07b1283a2e0782c96ad8fb1f9460351)), closes [#65](https://github.com/jolars/badness/issues/65)
- **lexer:** accept comment tail on macrocode end frame ([`fa01c29`](https://github.com/jolars/badness/commit/fa01c295e2faf5622d79eb5cd99564f491aa9bd5)), closes [#62](https://github.com/jolars/badness/issues/62)
- **parser:** gate `\begin`/`\end` on a name-shaped group ([`0dfeb0b`](https://github.com/jolars/badness/commit/0dfeb0bccbf820673369ce8d8fae13f6f1f4de6b))
- **parser:** gate text-mode brackets on a reachable closer ([`47f92f9`](https://github.com/jolars/badness/commit/47f92f9e89f8e29991c7658a2726b80465ac486e)), refs [#60](https://github.com/jolars/badness/issues/60)
- **lexer:** add l3doc to curated doc classes ([`e34cd7f`](https://github.com/jolars/badness/commit/e34cd7f495e3f2f211633cad6540605a3288e90d))
- **parser:** gate dollar math on a reachable closer ([`4a01a6b`](https://github.com/jolars/badness/commit/4a01a6bd1d600cb78fc6cbc6148ab33df5e67dcb))
- **formatter:** feed run-final trivia to expl3 run separator ([`ad2ce81`](https://github.com/jolars/badness/commit/ad2ce81c8dcc7209bef2350c201f904e0f7dca97)), closes [#58](https://github.com/jolars/badness/issues/58)
- **formatter:** own only macrocode bodies in dtx expl3 regions ([`d0abf21`](https://github.com/jolars/badness/commit/d0abf21e543d34df6b5571149d156a2c4daa8800))
- **formatter:** refine expl3 group style in regions ([`1cda543`](https://github.com/jolars/badness/commit/1cda5432d3fabeb1fd5f5c535350500bd46411f5)), refs [#57](https://github.com/jolars/badness/issues/57)
- parse doc short verbs and chunked dtx macro code ([`37fbf9c`](https://github.com/jolars/badness/commit/37fbf9c8d948c95b2ddd5a9cc75abe4bd0e8eb84)), refs [#57](https://github.com/jolars/badness/issues/57)
- **formatter:** strip script braces before operator atoms ([`d627794`](https://github.com/jolars/badness/commit/d627794ec713ade41984587fc8336177816873d1)), closes [#56](https://github.com/jolars/badness/issues/56)
- **parser:** count nested optionals in math bracket gate ([`7b98255`](https://github.com/jolars/badness/commit/7b982554f4bcd9a0eee2c282ce7240b4400ad71e)), closes [#55](https://github.com/jolars/badness/issues/55)
- **parser:** widen definition bodies to hooks and `\newcommand` ([`ab7d809`](https://github.com/jolars/badness/commit/ab7d80980a70a6b5f3c9ac0bf62dc5321cc2b5e3))
- **formatter:** keep trailing comments riding their line ([`71baa30`](https://github.com/jolars/badness/commit/71baa301673174f296f054e707040bcf40c616cf)), closes [#54](https://github.com/jolars/badness/issues/54)
- **parser:** make verb delimiter capture opt-in ([`4788af9`](https://github.com/jolars/badness/commit/4788af94ad7ad2744ec0f62b03ae941229821694)), closes [#53](https://github.com/jolars/badness/issues/53)
- **formatter:** keep own-line comments in list bodies ([`f468619`](https://github.com/jolars/badness/commit/f468619e986755bfad50331a5f85c43651f71e89)), closes [#48](https://github.com/jolars/badness/issues/48)
- **formatter:** keep grid indent on doc-commented rules ([`e0ccee0`](https://github.com/jolars/badness/commit/e0ccee0a5c23beb090a522f29f51edbc3d8bbfe6)), closes [#49](https://github.com/jolars/badness/issues/49)
- **formatter:** collapse fitting multi-line optional args ([`cf15d18`](https://github.com/jolars/badness/commit/cf15d1889b0313847f27df782ca152766e2c3456)), closes [#47](https://github.com/jolars/badness/issues/47)
- **formatter:** lift `\begin`-line `%` in every env layout ([`9c7fe0e`](https://github.com/jolars/badness/commit/9c7fe0e0343efc2b86ba97a029e09606d8d2b6a4)), closes [#38](https://github.com/jolars/badness/issues/38)
- **parser:** accept split `\begin`/`\end` in env definitions ([`e4f711d`](https://github.com/jolars/badness/commit/e4f711dc4224a5a9038604cdfc139d02fd0481d9)), closes [#45](https://github.com/jolars/badness/issues/45)
- **parser:** stop math brackets parsing as optional args ([`0eb49d5`](https://github.com/jolars/badness/commit/0eb49d549d1a5a4c3920fd9c97135665f051d80b)), closes [#43](https://github.com/jolars/badness/issues/43)
- **formatter:** improve display-math break-point detection ([`9e97826`](https://github.com/jolars/badness/commit/9e97826428c66055fb75516b910e2c15420fe76b)), refs [#42](https://github.com/jolars/badness/issues/42)
- **formatter:** keep multi-line math LHS off the relation column ([`b7fd62b`](https://github.com/jolars/badness/commit/b7fd62b0a92fd53246b1b0a5924f3aa7a2c75cba)), closes [#39](https://github.com/jolars/badness/issues/39)
- **formatter:** keep trailing `%` glued to a block segment ([`5369c22`](https://github.com/jolars/badness/commit/5369c224d0da45b5f04f62c2ad3bdbe45ae65d22)), closes [#38](https://github.com/jolars/badness/issues/38)
- **vscode:** swap npm-run-all for npm-run-all2 ([`b631714`](https://github.com/jolars/badness/commit/b631714b7e397493e7a703c44eb1ad12b9fa8da9))

## [0.11.0](https://github.com/jolars/badness/compare/v0.10.0...v0.11.0) (2026-07-20)

### Breaking changes
- **lsp:** move texmf config to editor settings ([`2f83a84`](https://github.com/jolars/badness/commit/2f83a844a6876906e69843053cafb59ed6e5232b))

### Features
- **lsp:** move texmf config to editor settings ([`2f83a84`](https://github.com/jolars/badness/commit/2f83a844a6876906e69843053cafb59ed6e5232b))
- **formatter:** hang nested blocks in align grids ([`5103bab`](https://github.com/jolars/badness/commit/5103babda2aa884f8dc5abe1b55a7fd2d3402fa5))
- **linter:** document bib rules in --explain and docs ([`a2a742c`](https://github.com/jolars/badness/commit/a2a742ccd9db648ae60d90359a8118364fdd60b7)), closes [#24](https://github.com/jolars/badness/issues/24)

### Bug Fixes
- **tests:** adapt to `lsp-server` 0.10 `Response` API ([`5f1e889`](https://github.com/jolars/badness/commit/5f1e889969c92a4d2f58fc3d4fb6365ed6b696d2))
- **linter:** skip script labels inside argument groups ([`1f8c0eb`](https://github.com/jolars/badness/commit/1f8c0ebed5e3ae93d617dc23195288446720ae46)), closes [#37](https://github.com/jolars/badness/issues/37)
- **parser:** name blank line as math terminator ([`2751787`](https://github.com/jolars/badness/commit/2751787acad3f69588eb66dfc562bb293e743a7f)), ref [#35](https://github.com/jolars/badness/issues/35)
- **linter:** ignore key arguments in dash-length ([`506e5f0`](https://github.com/jolars/badness/commit/506e5f0a365d1f5d988cdcd99706edab7ed435a8))
- **linter:** ignore rule-command spans in dash-length ([`adeecf6`](https://github.com/jolars/badness/commit/adeecf69d79cfb38d52f4187184104385feaad71)), closes [#34](https://github.com/jolars/badness/issues/34)
- **linter:** ignore key arguments in math-shape rules ([`387810a`](https://github.com/jolars/badness/commit/387810a7fbca8a9fd298634659c06dbb18ac0b6e)), closes [#25](https://github.com/jolars/badness/issues/25)
- **parser:** keep unmatched `[` a plain atom in math ([`c185c13`](https://github.com/jolars/badness/commit/c185c1338aa830646db53c344dd2638240ebbb0f)), closes [#23](https://github.com/jolars/badness/issues/23)
- **linter:** ignore labels in exclusive conditional branches ([`a6e0c22`](https://github.com/jolars/badness/commit/a6e0c228c177d25bd071a3ea8062ad8c9484c0aa))
- **linter:** ignore package loads in exclusive branches ([`d16d3d1`](https://github.com/jolars/badness/commit/d16d3d1a71c3abe6bca1fb78ee46e18d3b8aff33)), closes [#27](https://github.com/jolars/badness/issues/27)
- **linter:** target the whole construct for DOC_COMMENT-bound suppressions ([`cd647fa`](https://github.com/jolars/badness/commit/cd647fae218e61335ca080bda065d2c2b3425387)), fixes [#26](https://github.com/jolars/badness/issues/26)

## [0.10.0](https://github.com/jolars/badness/compare/v0.9.0...v0.10.0) (2026-07-15)

### Features
- add `--force-exclude` to `format` and `lint` ([`05c8e32`](https://github.com/jolars/badness/commit/05c8e32e7da31df9982c533b9f21369f049385c6))

## [0.9.0](https://github.com/jolars/badness/compare/v0.8.0...v0.9.0) (2026-07-14)

### Features
- **linter:** make fixes carry multiple atomic edits ([`7f52ef5`](https://github.com/jolars/badness/commit/7f52ef54bae11135d0e073978e73b25252f01d86))
- **linter:** add diagnostic related information ([`ada447b`](https://github.com/jolars/badness/commit/ada447bc4afe3001d1bafe96c5c759e240d5b1f8))
- **parser:** add a release-mode stuck-loop step limiter ([`f93b1e5`](https://github.com/jolars/badness/commit/f93b1e5043125802a77da462e8305b8748a25036))
- **lsp:** add diagnostic tags and rule doc links ([`906b0df`](https://github.com/jolars/badness/commit/906b0dfbd43477f8320a06dd1f4807207bfd7e66))

### Bug Fixes
- **lsp:** recover poisoned db mutexes instead of panicking ([`5844ff2`](https://github.com/jolars/badness/commit/5844ff2c029b207cf8e9fa55124d68ca831c953c))
- **bib:** resolve field aliases in missing-required check ([`f20283b`](https://github.com/jolars/badness/commit/f20283baabca65eeaadf447ef80bba7ded5de81e))

## [0.8.0](https://github.com/jolars/badness/compare/v0.7.0...v0.8.0) (2026-07-11)

### Features
- **lsp:** show source package in macro hover ([`eaf030e`](https://github.com/jolars/badness/commit/eaf030ea9da011ce90b77b64f37155ffab5da665))
- **lint:** add `unknown-option` rule for local packages ([`f040033`](https://github.com/jolars/badness/commit/f040033e049acb340f27e66eba1eeecc70f68a33))
- **bib:** document links for doi and url fields ([`e1f15f7`](https://github.com/jolars/badness/commit/e1f15f7f8d0b1afe265f1a5c40f7a28f88437b8f))
- **completion:** argument-value enum completion ([`f363499`](https://github.com/jolars/badness/commit/f36349970afdb020af52e368051db11d3331b58b))
- **bench:** add linter speed benchmark vs lacheck and chktex ([`e6a821e`](https://github.com/jolars/badness/commit/e6a821ed159211081fce7030bef567d4f7ae371e))
- **bench:** add whole-project folder benchmark ([`620c7cc`](https://github.com/jolars/badness/commit/620c7cc5202b47c017ab4135184cd8d54ad5ba76))
- **bib:** typed AST wrapper layer for BibTeX CST ([`0abe33a`](https://github.com/jolars/badness/commit/0abe33ac3f68f77c390328fc2d7da804cd235f39))
- **ast:** typed AstNode/AstToken wrapper layer ([`35eae44`](https://github.com/jolars/badness/commit/35eae44b0fd70850aa1e02e8204c098479e6d391))

### Performance Improvements
- **cli:** parallelize lint --fix across files ([`133a4c3`](https://github.com/jolars/badness/commit/133a4c39ff6a34665e5fa6552cc13f9dc7253a23))

## [0.7.0](https://github.com/jolars/badness/compare/v0.6.0...v0.7.0) (2026-07-08)

### Features
- **lsp:** selection ranges from CST hierarchy ([`2aff55f`](https://github.com/jolars/badness/commit/2aff55f151c5b4b593b62dfa9e3f817ea5602db0))
- **formatter:** column-spec-aware table alignment ([`9ab94ba`](https://github.com/jolars/badness/commit/9ab94ba19ce8c34f13cff42dd248d78a570961c0))
- **linter:** package-aware duplicate and provides lints ([`758fac3`](https://github.com/jolars/badness/commit/758fac39180017423444d2ba57ca9d38031734b4))
- **semantic:** recognize package metadata and options ([`0cd95ce`](https://github.com/jolars/badness/commit/0cd95ce51a58c87a19b6c9564d2e6eea3307f476))
- **lsp:** color and TikZ/PGF library completion ([`1a881f3`](https://github.com/jolars/badness/commit/1a881f33a598be78e6e196ac53733c2b584bcbed))
- **lint:** add unreferenced-label rule ([`4d975a1`](https://github.com/jolars/badness/commit/4d975a1560b8a67b6befb505f1470657f30dee30))
- **lint:** add verbatim-trailing-text rule ([`a11358d`](https://github.com/jolars/badness/commit/a11358dff3bcd09b2d27c8ae373bd6e190dc1aa5))
- **lint:** flag line-break tie in missing-nonbreaking-space ([`de2d51f`](https://github.com/jolars/badness/commit/de2d51fb1db7d0c53d6f868cec13bff51f4df6c7))
- **lint:** autofix obsolete-environment eqnarray to align ([`aa26b13`](https://github.com/jolars/badness/commit/aa26b138e3f19a83f42541401de20d4b1bf1690e))
- **lint:** add missing-required-argument rule ([`5206ee6`](https://github.com/jolars/badness/commit/5206ee66e6b683b371c0fc670440bb981e92ceec))
- **lsp:** references, rename, goto-def for user macros ([`fdeb0e9`](https://github.com/jolars/badness/commit/fdeb0e9b3f2c35076656b61b1022b7ffcf7865ab))
- **lsp:** negotiate client capabilities at initialize ([`36b6ed2`](https://github.com/jolars/badness/commit/36b6ed29d40f61839ccc43ec8f5657a10d27b50d))
- **lsp:** change-environment refactor command ([`1f27fab`](https://github.com/jolars/badness/commit/1f27fab4ccc37022555588dac39fe56566afefb7))
- **lsp:** glossary/acronym key completion ([`f73f138`](https://github.com/jolars/badness/commit/f73f138c46fbb8982aa580528114be70e28f2c75))
- **lsp:** signature help for command arguments ([`0c5f649`](https://github.com/jolars/badness/commit/0c5f6491d7b377db7e04da2daacbbaa8476c5b75))
- **lsp:** label hover and symbol numbers from `.aux` ([`3efb7aa`](https://github.com/jolars/badness/commit/3efb7aa4d7afc64ecb3ba340097d3f78997062ce))
- **semantic:** classify what a `\label` labels ([`01a8b0b`](https://github.com/jolars/badness/commit/01a8b0b76b408c5d0da48a75dfeedb67a1bf8697))
- **project:** scan `.aux` for label numbers and toc ([`ed48898`](https://github.com/jolars/badness/commit/ed4889826dd9be877a8eef425060c3186b5e9f82))
- **config:** add `[build]` section with `aux-dir` ([`ad8cc8a`](https://github.com/jolars/badness/commit/ad8cc8a80bc8932feb01844616f776aec273afa7))
- **lsp:** go-to-definition for include/package file arguments ([`99927ea`](https://github.com/jolars/badness/commit/99927ea832d901f4c8810f0611cd4350e3871bea))
- **lsp:** resolve packages via TEXMF index and CTAN metadata ([`24ba5c7`](https://github.com/jolars/badness/commit/24ba5c73c955544471a29553a97f91695b0ae48a))

### Bug Fixes
- **bib:** tighten title-capitalization camelCase heuristic ([`91de065`](https://github.com/jolars/badness/commit/91de06585001800aab0bf3ff2b91b3e126e7c244))
- **ci:** rename aux.rs, allow option-ext MPL-2.0 ([`cc1c834`](https://github.com/jolars/badness/commit/cc1c8348058f6b1856af98874e1d77163c12f882))

### Performance Improvements
- **formatter:** parallelize the CLI format paths ([`d38b4d6`](https://github.com/jolars/badness/commit/d38b4d6bc2b2899a25b0b5ad970895b438d5e9da))
- **linter:** cache registry, stream rewalkers, parallelize CLI ([`5c3813a`](https://github.com/jolars/badness/commit/5c3813a298d4c2c3e1e9771b806385c5be6d6a74))
- **signature:** bake CTAN metadata via phf, not runtime parse ([`f635d23`](https://github.com/jolars/badness/commit/f635d23f215516269485b9fbfc73a289e84c0317))

## [0.6.0](https://github.com/jolars/badness/compare/v0.5.0...v0.6.0) (2026-07-06)

### Features
- **completion:** complete `\usepackage`/`\documentclass` names ([`2457147`](https://github.com/jolars/badness/commit/24571476c68f47e2acdd0f0ed918f0b2cb584e04))
- **completion:** add baked package/class name lists ([`ff4906d`](https://github.com/jolars/badness/commit/ff4906dfd2e4dbf3aa2830eb382c63284429cf69))
- **lsp:** add document links ([`915aea6`](https://github.com/jolars/badness/commit/915aea6402fa4813c5eae383548d300b1f3610da))
- **lsp:** highlight matching `\begin`/`\end` pair ([`d643518`](https://github.com/jolars/badness/commit/d64351844066b7232c71460528b7391e3a538b28))
- **lsp:** re-indent on close via onTypeFormatting ([`5972340`](https://github.com/jolars/badness/commit/5972340cb4d8bb1bc9972f55b559e4211f1e7228))
- **parser:** parse math environments in math mode ([`9097be3`](https://github.com/jolars/badness/commit/9097be3717c025b73bcc528dc2ac35b13bcd6b94))
- **formatter:** implement sentence and semantic wrap modes ([`17003ba`](https://github.com/jolars/badness/commit/17003bace4b7a40505361adfbd6545b58ee66b6d))
- **linter:** add hard-coded-reference rule ([`da66c29`](https://github.com/jolars/badness/commit/da66c298c385142a76970b91a61e322f11e2b765))
- **linter:** add sectioning-level-jump rule ([`6ac6def`](https://github.com/jolars/badness/commit/6ac6defb60760c257f51ab2e7f22884ac3fc2b0d))
- **linter:** add makeat-macro rule ([`2ae6d07`](https://github.com/jolars/badness/commit/2ae6d075ab6ab01e7efe44cad5f538c4292f7fd2))
- **linter:** add space-before-command rule ([`36d5fa3`](https://github.com/jolars/badness/commit/36d5fa3c47af7939bcbacba8911bab3ca118b485))
- **linter:** add abbreviation-spacing rule ([`2fea8db`](https://github.com/jolars/badness/commit/2fea8db340303dbb5f640adc7fc5ddc83aacfd74))
- **linter:** add swallowed-space rule ([`c48aa20`](https://github.com/jolars/badness/commit/c48aa20ce6f59df9cca16959417abf6938f1a5b3))
- **linter:** add primitive-command rule ([`94da7ca`](https://github.com/jolars/badness/commit/94da7cacda77e12ed0da2fa323738920fe681fdf))
- **linter:** add math-operator-name rule ([`17cc5f2`](https://github.com/jolars/badness/commit/17cc5f2e175315d5686d0f9ed1cea781e48f82f3))
- **linter:** add times-variable rule ([`52de07a`](https://github.com/jolars/badness/commit/52de07ac61fb5222ef4b4d1a3c0cd0631c40ebd3))
- **linter:** add dash-length rule ([`a6218e0`](https://github.com/jolars/badness/commit/a6218e0aadde3694786f58132104b5d23fd8c5c6))
- **linter:** add straight-quotes rule for ASCII quotes ([`adff4ba`](https://github.com/jolars/badness/commit/adff4bafdfa91f6e8f2f0598e32430248b48d9cb))
- **linter:** add ellipsis rule for literal ... ([`488ebdd`](https://github.com/jolars/badness/commit/488ebddc399f91acc29f5cb38aa851ac962afd57))
- **linter:** generate rules reference from metadata ([`74e2234`](https://github.com/jolars/badness/commit/74e223425ada75b25e3558903784cf2cf2a9c438))
- **math:** normalize operator spacing ([`36c9314`](https://github.com/jolars/badness/commit/36c9314b3017366879160a385a67e83f3bdcbead))
- **semantic:** keep built-in over delegating arity-0 redef ([`9fd50d8`](https://github.com/jolars/badness/commit/9fd50d8f74f661dffa8f9e3bbb6c58494d2453df))
- add title, author, date, thanks to signatures db ([`3c537d1`](https://github.com/jolars/badness/commit/3c537d1b3b4af9f1f7ce1abd6a06914504dd03a8))
- **formatter:** stack binary chains under the relation too ([`0777920`](https://github.com/jolars/badness/commit/077792009dc99f761a0046012cffdb8bdf8e04b6))
- **formatter:** align relation chains in display math ([`e69a72e`](https://github.com/jolars/badness/commit/e69a72ee540ab9af778cbd7246d0e09351d7bc82))
- **formatter:** join alignment-cell continuation lines ([`cd3e590`](https://github.com/jolars/badness/commit/cd3e5907c7b629348222306badce7501aadb4f41))
- **semantic:** resolve packages to .dtx sources ([`249e68e`](https://github.com/jolars/badness/commit/249e68e63bc91844a863e123767e06d2daf5aed9))

### Bug Fixes
- **parser:** point unclosed-delimiter errors at the opener ([`1029351`](https://github.com/jolars/badness/commit/10293517fb100519c84cb830b446327be06ded8e))
- **formatter:** tight spacing and no paren breaks in display math ([`7112b8c`](https://github.com/jolars/badness/commit/7112b8cd9b4b6e0d9d5e230b9fe75c8a587079d7))
- **linter:** allow en dash between proper names in dash-length ([`2ab4342`](https://github.com/jolars/badness/commit/2ab4342147279ec63c86b5f26e53a8749805ec9d))
- **formatter:** peel over-attached cell off table rules ([`7c91ac9`](https://github.com/jolars/badness/commit/7c91ac97cd1ff7e74b4c4a629ef4dab713307334))

### Reverts
- "feat(formatter): stack binary chains under the relation too" ([`4a6988b`](https://github.com/jolars/badness/commit/4a6988bbebf2257a4cdf2c05c86c33bd561fd7be))

## [0.5.0](https://github.com/jolars/badness/compare/v0.4.0...v0.5.0) (2026-07-01)

### Features
- **lsp:** add range formatting support ([`5ad2827`](https://github.com/jolars/badness/commit/5ad2827e9ecd7046bd9465ee965fb5666d1ffe28))
- **lsp:** add workspace symbols support ([`eb8a111`](https://github.com/jolars/badness/commit/eb8a111c18d346f73e34dcec39201ad28bf51da4))
- **formatter:** format expl3 code (catcode 9/10 model) ([`ac4ff31`](https://github.com/jolars/badness/commit/ac4ff313fdad326bd0bf854b1f268c4d7d4b580a))
- **lsp:** watch on-disk tex/bib/config and reanalyze ([`b551c01`](https://github.com/jolars/badness/commit/b551c0123d8a0cc174cbb8f3612c04ef883af371))
- **dtx:** reflow documentation prose under reflow ([`be57646`](https://github.com/jolars/badness/commit/be576463c1bb4c32972dc47fc306065c43a991c1))
- **lsp:** outline entries for dtx documented macros ([`cba0b01`](https://github.com/jolars/badness/commit/cba0b01003f5056ccfca0f2c5e9297bd3247969c))
- **lsp:** add `textDocument/documentHighlight` ([`404069b`](https://github.com/jolars/badness/commit/404069b84ad75450f500378c842efe38fa7e3ba3))
- **bench:** add formatter speed bench vs tex-fmt & latexindent ([`82ddeb5`](https://github.com/jolars/badness/commit/82ddeb54ff218882ad3e1a1fd6228af1cd3a8081))
- **format:** reflow brace-group bodies as statements ([`bb976e0`](https://github.com/jolars/badness/commit/bb976e09cd701e315767cdcbd0b0bae6b536c1bc))
- **lsp:** discover and apply badness.toml per document ([`e56a8af`](https://github.com/jolars/badness/commit/e56a8afc7621cd8e37df5848ab1a382739991e3e))
- **lint:** add missing-nonbreaking-space (tie before cite/ref) ([`4d75da4`](https://github.com/jolars/badness/commit/4d75da4b9459361569f6d2407e5bdb675163c4ea))
- **lsp:** surface linter autofixes as code actions ([`13c727e`](https://github.com/jolars/badness/commit/13c727e339d5e4c1f8e848caaf307b1fe9c9eb27))
- **lsp:** resolve completion items with signature and citation detail ([`f9892e6`](https://github.com/jolars/badness/commit/f9892e62fb6ca1c804bc6f2912ea581a37d6b0bc))
- **lsp:** add hover for commands, environments and citations ([`3c6047c`](https://github.com/jolars/badness/commit/3c6047cdae637bd18fb6216a8c1f52ec06460157))
- **lsp:** add pull diagnostics ([`a73fd7b`](https://github.com/jolars/badness/commit/a73fd7b8f677b8fdca27be2d68bc3701beaaffc1))

### Bug Fixes
- **lsp:** honor excludes for siblings ([`7a50529`](https://github.com/jolars/badness/commit/7a5052953c3ef58174fa0f7f55f640a216c23428))

### Performance Improvements
- **signature:** bake CWL tier into a build-time phf map ([`a920d4a`](https://github.com/jolars/badness/commit/a920d4a803dcd89ab97e7dd7945148c95958de2c))

## [0.4.0](https://github.com/jolars/badness/compare/v0.3.0...v0.4.0) (2026-06-23)

### Features
- **semantic:** mark the cross-reference family inline ([`c7c77a7`](https://github.com/jolars/badness/commit/c7c77a7aa96123c51fafb0fe0bf6e3e9e1a07aef))
- **semantic:** ingest CWL corpus as a bulk signature tier ([`4740bf5`](https://github.com/jolars/badness/commit/4740bf50d00fea3bdb5cd90e6c9de2924da051ec))
- **lint:** don't withold lints that disturbs alignment ([`8ea1efc`](https://github.com/jolars/badness/commit/8ea1efccecda47fd2dc70324e5104356ca047505))
- **bib:** diagnose missing field separator; fix value trivia attachment ([`e14751c`](https://github.com/jolars/badness/commit/e14751cfc096dd264eb29848511be21c4d13738d))
- **bib:** autofix duplicate-field when values are identical ([`c34bd78`](https://github.com/jolars/badness/commit/c34bd780d2d19486ea3d99be61c9d409125148ce))
- **bib:** duplicate-field lint rule ([`f2f6d60`](https://github.com/jolars/badness/commit/f2f6d60763077e165d292d416dd4171fbe60bad0))
- **lsp:** rename labels and citation keys (textDocument/rename + prepareRename) ([`7b1d01b`](https://github.com/jolars/badness/commit/7b1d01b68e4c900a17402998919147594f0b235d))
- **config:** badness.toml configuration (CLI) ([`8c68ca2`](https://github.com/jolars/badness/commit/8c68ca2fa47f3907bc98d8415c1246ac7a4755b7))
- **project:** package load graph + package signatures into scope ([`f8e6bc7`](https://github.com/jolars/badness/commit/f8e6bc7f661c26be6fad15821bda09ce85168a43))
- **semantic:** doc/ltxdoc prose↔code association query ([`a52f17c`](https://github.com/jolars/badness/commit/a52f17c5f4e22ea984ee92cd9cff9e8fc61ded4b))
- **file-kind:** .ins installation-script support (plain code, Preserve) ([`85c9c7a`](https://github.com/jolars/badness/commit/85c9c7a73f7d208e1e1df909647229f04f5ec115))
- **formatter:** .dtx two-layer formatting (foundation, Preserve) ([`6c7861f`](https://github.com/jolars/badness/commit/6c7861f1670f8222d36c87f645766c9ba3ef4a5b))
- **semantic:** doc/ltxdoc signatures + DOC_COMMENT node (M3) ([`95ec2a2`](https://github.com/jolars/badness/commit/95ec2a24bbb88d28fc4394abc0137182b7780f5b))
- **parser:** lex expl3 syntax mode (_/: as letters) ([`c98e2e8`](https://github.com/jolars/badness/commit/c98e2e84ad78cc3413db1ba0b0eb8bf13be53d86))
- **parser:** lex .dtx docstrip guards as GUARD tokens (M2) ([`b09c507`](https://github.com/jolars/badness/commit/b09c507c3e3901f52e2d12c0c3c87976b0712c99))
- **parser:** parse .dtx docstrip surface syntax (M0+M1) ([`8e54604`](https://github.com/jolars/badness/commit/8e54604fbd80fcfd0e585d927e54f1096a086e36))
- **lsp:** add textDocument/foldingRange ([`f0ea513`](https://github.com/jolars/badness/commit/f0ea51356645dce2076ea23c55386e34e9979bac))

### Bug Fixes
- **cli:** fix file-detection in cli linter ([`7821b6a`](https://github.com/jolars/badness/commit/7821b6ae35cca242933d862b59206b5443261c8b))

## [0.3.0](https://github.com/jolars/badness/compare/v0.2.0...v0.3.0) (2026-06-21)

### Features
- **lsp:** add textDocument/references (find references) ([`2ef3606`](https://github.com/jolars/badness/commit/2ef3606ac2bf343d6eac4712980bbed6f7016c1b))
- **sty/cls:** format and lint LaTeX package/class sources ([`54692cf`](https://github.com/jolars/badness/commit/54692cf18b33a834ab1836f4841e105d92915cbc))
- **lsp:** bib-aware completion and \cite key completion ([`493ad41`](https://github.com/jolars/badness/commit/493ad419d6c0923e360b35f8e9736e8c72ea75cb))
- **bib:** add generator to sync bib_fields.json with biblatex data model ([`189de08`](https://github.com/jolars/badness/commit/189de08b544231233f8551f208ffee43ed93dc74))
- **bib:** align entry-type required fields to the data model ([`35b81d9`](https://github.com/jolars/badness/commit/35b81d920ef002e2927ade1bf7e0ebc2d9773eba))
- **bib:** derive field/entry DB from biblatex's canonical data model ([`55a6883`](https://github.com/jolars/badness/commit/55a6883b89bdd9ebc2bcbbf9b60feb60e58038db))
- **bib:** recognize the full standard biblatex field set ([`e2c2639`](https://github.com/jolars/badness/commit/e2c263919cf5a1be6c5d3959af8f8dd1606fc7c6))
- **semantic:** flag user verbatim environments via begin-code catcode scanning ([`eefc1a1`](https://github.com/jolars/badness/commit/eefc1a1ecc92f06600d5142878d74f03e50e213b))
- **semantic:** scan \def-defined verbatim commands and helper chains ([`6cad9c1`](https://github.com/jolars/badness/commit/6cad9c13e208361db03f0adcf47a37f1f3371edf))
- **semantic:** flag user verbatim-argument commands via definition scanning ([`19ef5f1`](https://github.com/jolars/badness/commit/19ef5f164dcbf751d123f31489fc0c7ac0754e24))
- **lsp:** go-to-definition for refs and citations ([`2535199`](https://github.com/jolars/badness/commit/25351994cfaf057b82dba03954127adf02bb546b))
- **cli:** --stdin-filepath routes lint stdin to the bib pipeline ([`f8a4831`](https://github.com/jolars/badness/commit/f8a48311b6624af5eadce702a52957b8a0281b1a))
- **cli:** --stdin-filepath routes format stdin to the bib pipeline ([`96f1b80`](https://github.com/jolars/badness/commit/96f1b80b7b83afb99bc9cc29d5cdc5223b5ed18d))
- **lsp:** cross-file project assembly — undefined-ref/citation fire live ([`38b7f2c`](https://github.com/jolars/badness/commit/38b7f2c5a11c7c75fda5039aa002dc355317357f))
- **bib:** Phase 4 — incremental, LSP, and project-graph integration ([`b593bdc`](https://github.com/jolars/badness/commit/b593bdce12cb6d10839be1da7bdfa4ed829b70d5))
- **bib:** linter rules + CLI wiring (Phase 3) ([`571c2d3`](https://github.com/jolars/badness/commit/571c2d3daf6abafa5aff20aba5fcb54eaad649eb))
- **bib:** field & entry sorting (Phase 2c) ([`438a61d`](https://github.com/jolars/badness/commit/438a61d7b90688e5b59b84f005de52df7d187a1a))
- **bib:** value reflow (Phase 2b) — wrap long field values by category ([`3cfed27`](https://github.com/jolars/badness/commit/3cfed2770a4eddab413f6649b1f1bf0f9996f9fe))
- **bib:** formatter (Phase 2) — lower bib CST to shared Wadler IR ([`de48afd`](https://github.com/jolars/badness/commit/de48afdf930736c2a1086e5621175f8c4353daa2))
- **bib:** semantic model + field/entry signature DB ([`b59befc`](https://github.com/jolars/badness/commit/b59befc3562093930265da5c93c0da43f147cf62))
- **bib:** differential parse oracle vs texlab + phased roadmap ([`d7360b6`](https://github.com/jolars/badness/commit/d7360b6d7b03092e47db800a14752f3ca2889e52))
- **bib:** first-stab BibTeX/BibLaTeX parser ([`6f38675`](https://github.com/jolars/badness/commit/6f38675001647f04775f302ae0c394a24125b9c7))
- **lsp:** add basic completion ([`20903b7`](https://github.com/jolars/badness/commit/20903b7922a0858266a9b62d871ef13e866789e5))
- **linter:** autofix infra + dollar-display-math $$→\[ fix ([`216f590`](https://github.com/jolars/badness/commit/216f5909ca6e301ebca24cb2b81ab9d558985a25))
- **linter:** obsolete-environment, dollar-display-math, mismatched-delimiter lints ([`8f89b51`](https://github.com/jolars/badness/commit/8f89b510094493d84a44f1946304d71072564f62))
- **formatter:** break wide display math at top-level operators ([`716612f`](https://github.com/jolars/badness/commit/716612f309d6766dfcef7654e8d276f754eeac56))
- **linter:** cross-file label resolution + undefined-ref / duplicate-label ([`270a035`](https://github.com/jolars/badness/commit/270a0357a11dcf99bad0e14dc723fdb3d7eddf2a))
- **formatter:** keep appendix environment body flush like document ([`b1a55f7`](https://github.com/jolars/badness/commit/b1a55f7fe6c627b3a061ee9083b7ee1a821678ba))
- **formatter:** collapse cite-family key lists deterministically ([`d88e7e3`](https://github.com/jolars/badness/commit/d88e7e32f67b3959e866e15d51b3ddf379604ec3))
- **semantic:** extract unbraced \newcommand\foo definition form ([`f2472d5`](https://github.com/jolars/badness/commit/f2472d53ec48738eb11f27e817f395e84f9e7278))
- **parser:** bind leading comments into the following construct ([`0afabeb`](https://github.com/jolars/badness/commit/0afabeb177730fac82d0ed33e0dc7c6b40959050))
- **lsp:** add document symbols ([`5547650`](https://github.com/jolars/badness/commit/5547650f7debe5ff1d629b4a716fed70698924fb))
- **parser:** don't wrap a lone block environment in a PARAGRAPH ([`b4a46fe`](https://github.com/jolars/badness/commit/b4a46fedbe57d96550642769b37a80d0fa8515da))
- **formatter:** use latexindent-style desc hang ([`46ab231`](https://github.com/jolars/badness/commit/46ab23176980853e39fbd3b6525c2ca44a577ee6))
- **formatter:** reflow inline prose commands inline, not as blocks ([`5d706b2`](https://github.com/jolars/badness/commit/5d706b260a26e3d3f62258ff3e3044faac02f6a0))
- collapse blanklines into 1 ([`b19d8da`](https://github.com/jolars/badness/commit/b19d8da54b125067153850d55674486673bca2a5))
- **formatter:** grid-align comments and rule lines; enable tables ([`4cbb183`](https://github.com/jolars/badness/commit/4cbb1836e814a2ed4611336f005a2fc5d17d48c9))
- **formatter:** lower display math as an indented block ([`5e2cefc`](https://github.com/jolars/badness/commit/5e2cefc9bcd4faf09ef6483cccf89e3ca85827fd))
- **parser:** lex verbatim-argument commands; fix multi-line VERB formatting ([`73cf04c`](https://github.com/jolars/badness/commit/73cf04c6afdaab38088360c1f279c9c6496b9138))
- **cli:** add badness parse command ([`7735a75`](https://github.com/jolars/badness/commit/7735a75f3f78897be47961e7b5a48c053318526e))
- **formatter:** align itemize blocks ([`47a2b19`](https://github.com/jolars/badness/commit/47a2b199525a17bb9a1f4bffde833e7d2433ed1a))
- **formatter:** don't indent document environment ([`3cd0d04`](https://github.com/jolars/badness/commit/3cd0d043e5fa2d2fa8e008986e896ebb32bd4a14))
- **linter:** add rule layer with duplicate-label and deprecated-command ([`4aaee37`](https://github.com/jolars/badness/commit/4aaee372a4bdf33b6a6edd24948cf5b721b97280))
- align & columns in align/matrix environments ([`d5abdca`](https://github.com/jolars/badness/commit/d5abdca6319aed86cca105ed5c5c06e8ccaf9d0a))
- match \left … \right delimiter pairs in math ([`3079875`](https://github.com/jolars/badness/commit/30798752941eb6956592a572572a0f6130a49002))
- add structured math model and math formatting ([`02802f6`](https://github.com/jolars/badness/commit/02802f60121ac265e6ef4b2a2b6549577b6ad62e))
- support argument-taking verbatim environments ([`ab8eb74`](https://github.com/jolars/badness/commit/ab8eb74a744d546c6dcfc09e6a3d7df11a369ca1))
- add file-walk for formatter ([`1603230`](https://github.com/jolars/badness/commit/16032307362a53e5cf8379e44c6428b0493a5e8a))

### Bug Fixes
- **lsp:** handle Windows file URIs in path completion ([`5b38f45`](https://github.com/jolars/badness/commit/5b38f45de7a6b122db90956ca4dfd6375099319c))
- **formatter:** keep a trailing % on the \begin header line ([`e02413f`](https://github.com/jolars/badness/commit/e02413f7c7c521eb5d87f8382c43078c2adadb3c))
- **formatter:** ass JSS/Sweave verbatim environments to signatures ([`21b5e61`](https://github.com/jolars/badness/commit/21b5e616d1a2d9ccf0396f02295dbae586a88a34))
- don't reflow single `%` ([`be49170`](https://github.com/jolars/badness/commit/be49170135a3fb549480e5367f0a7ed9232edbd0))
- **formatter:** don't push `%` to next line ([`de271ae`](https://github.com/jolars/badness/commit/de271aef11a78f5febc41374f95a81f4c6091be6))
- **formatter:** fall back when an alignment cell contains a comment ([`918c592`](https://github.com/jolars/badness/commit/918c592bec5116bdbe84a12f99dfe08c66a45a0e))
- **parser:** don't treat comment-only lines as paragraph breaks ([`3c83c01`](https://github.com/jolars/badness/commit/3c83c01904fc679e7b1175c6ec57ff0d0015daf5))
- **formatter:** keep command-only lines on their own line under reflow ([`739a32f`](https://github.com/jolars/badness/commit/739a32fe545abcca0c6e2de8338b825419f67a5e))
- **linter:** migrate render.rs to annotate-snippets 0.12 API ([`602d835`](https://github.com/jolars/badness/commit/602d835163e13736525682cf6994b70152035341))

## [0.2.0](https://github.com/jolars/badness/compare/v0.1.0...v0.2.0) (2026-06-12)

### Features
- add vscode and open vsx extensions ([`975f1e4`](https://github.com/jolars/badness/commit/975f1e49428a3026b53382efcf02ef65996b4d47))
- **npm:** package for npm ([`b3a576f`](https://github.com/jolars/badness/commit/b3a576fa970d2d07de9521a7a3c5f16c13c535d6))

## [0.1.0](https://github.com/jolars/badness/compare/v0.0.1...v0.1.0) (2026-06-12)

### Breaking changes
- rename fmt to format ([`1fedc1b`](https://github.com/jolars/badness/commit/1fedc1b65c32933fb5dc649e7dcc2307d7ea60cf))

### Features
- **formatter:** reflow signature-marked prose arguments ([`18c99ee`](https://github.com/jolars/badness/commit/18c99ee168976258c32310093fa5267180510221))
- **lsp:** ra-style writer/threadpool, cancellation, incremental sync ([`8628f92`](https://github.com/jolars/badness/commit/8628f928e8a65fd758d4badf4e5035daf6270cf2))
- **lsp:** reuse cached salsa tree for formatting ([`30cd2d5`](https://github.com/jolars/badness/commit/30cd2d5822a7b7321b2025421312bdcc8eef5b92))
- implement semantic group scanning ([`4f5e9ca`](https://github.com/jolars/badness/commit/4f5e9caafbc8fab60729ea6b925d7a6c14b750a8))
- **parser:** model \\ line break as a LINE_BREAK node ([`651e1c5`](https://github.com/jolars/badness/commit/651e1c5552f70ee59eeecf0da87b45e134b9d20a))
- **formatter:** paragraph reflow via a Wadler Fill node ([`0cbe264`](https://github.com/jolars/badness/commit/0cbe264134d4f820b0181efc98e77291ffbf6b74))
- **semantic:** add built-in signature database ([`e9bf2de`](https://github.com/jolars/badness/commit/e9bf2de6b4c5c8c77cdd6dffa495c25b43c68645))
- rename fmt to format ([`1fedc1b`](https://github.com/jolars/badness/commit/1fedc1b65c32933fb5dc649e7dcc2307d7ea60cf))
- **linter:** add minimal `badness lint` command ([`443fa6a`](https://github.com/jolars/badness/commit/443fa6a652ee4b0fb1a0f3b9e91d430cf0e13f15))
- **lsp:** add minimal lsp server ([`7e6f4fe`](https://github.com/jolars/badness/commit/7e6f4fe03d9e1290f092f69b295e684c63cf78f8))
- **formatter:** indent multi-line group/argument bodies ([`5e66038`](https://github.com/jolars/badness/commit/5e6603832e79955bb9b65149c78263bad9b4e8a0))
- **parser:** differential parse oracle vs texlab ([`25e065c`](https://github.com/jolars/badness/commit/25e065c7bdce2b2c70d4fffa1916cbf4e6650a07))
- **lsp:** add semantic model and reference support ([`61707c1`](https://github.com/jolars/badness/commit/61707c151ff18ea5e469c00e14ce27f978a9f801))
- build project graph ([`cc81a29`](https://github.com/jolars/badness/commit/cc81a291ed03d1149a53d95270f0f56bdba697d8))
- **incremental:** salsa harness for cached parsing ([`67a1948`](https://github.com/jolars/badness/commit/67a194841890ba3fa582d8302cfc8bc446077412))
- **formatter:** environment-body indentation ([`5b3d1b5`](https://github.com/jolars/badness/commit/5b3d1b5b270f24deb32898309e2b46afc0ecd7f3))
- **formatter:** whitespace normalization (first real rule) ([`00385eb`](https://github.com/jolars/badness/commit/00385eb6707fe7c916d4626f4366d989878ce422))
- **formatter:** Phase 2 formatter MVP — identity round-trip ([`ab2ef57`](https://github.com/jolars/badness/commit/ab2ef572d2a21addf89e6a8ba9448cefc44b02cc))
- **parser:** Phase 1 recursive-descent grammar with error recovery ([`511352c`](https://github.com/jolars/badness/commit/511352c643c87992e3c285cc77ed7c6f4579af50))

### Bug Fixes
- attach arguments to environment ([`a6772d2`](https://github.com/jolars/badness/commit/a6772d21fa7f25a3c35ab00f8d4667146767eaed))
- **parser:** stop $-math at group and \end anchors ([`1319fd8`](https://github.com/jolars/badness/commit/1319fd8e6ae834e2023e6c0b1d9c9e5adc9781ca))
