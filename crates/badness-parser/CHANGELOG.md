# Changelog

## [0.4.0](https://github.com/jolars/badness/compare/badness-parser-v0.3.0...badness-parser-v0.4.0) (2026-08-24)

### Features
- add LSP memory benchmark ([`9a975d7`](https://github.com/jolars/badness/commit/9a975d7dcee69bc143a90ecff041b36d5de356c2))
- **parser:** reparse math fragments ([`4638eb5`](https://github.com/jolars/badness/commit/4638eb5a644019235ef976420e58f47834354fc2))
- add semantic math atom classification ([`3889311`](https://github.com/jolars/badness/commit/388931180fec563d358aef9d2d20f8d6993ddaf1))
- **parser:** add argument domains ([`f7f4b01`](https://github.com/jolars/badness/commit/f7f4b011661f0106028c5e75896096b9b310f4d4))

### Bug Fixes
- **parser:** consume CRLF control symbols atomically ([`20dd182`](https://github.com/jolars/badness/commit/20dd18273e2aec5862abbb5d2d2f945b74467968))
- **parser:** parse alignment char constants ([`78f3cd9`](https://github.com/jolars/badness/commit/78f3cd962b17379141efda3067495df8723f1b67))
- **parser:** guard expl3 mode boundaries ([`08c0387`](https://github.com/jolars/badness/commit/08c0387ec0cad42ad04782d2175f5e61ef294fa3))
- **parser:** unwind environment gate mismatches ([`6398829`](https://github.com/jolars/badness/commit/6398829dde0e22ed7680ca270f846fffd30c5d8d))
- **parser:** gate math environments in groups ([`b221ccd`](https://github.com/jolars/badness/commit/b221ccda0f983ad3ce512d59d8152f63b667a2ad))
- **parser:** tighten catcode signal ([`834c7b6`](https://github.com/jolars/badness/commit/834c7b65b6ab319430e1ed86fdb62582f3b61812))
- **parser:** pass plain braces through optionals ([`82b30f1`](https://github.com/jolars/badness/commit/82b30f17190f935432c3df59fc6428ed33003562))
- **parser:** parse href URLs verbatim ([`03ad84f`](https://github.com/jolars/badness/commit/03ad84f6d78307d155a543b52047e6fc33591d51))
- **parser:** preserve argument mode semantics ([`63aefa8`](https://github.com/jolars/badness/commit/63aefa8e626f9e7e1012ca02ab0370440441c5b2))
- **parser:** honor TeX script atom boundaries ([`e6131b6`](https://github.com/jolars/badness/commit/e6131b69e1df9f3fed25af772fce803655b3e88d))

## [0.3.0](https://github.com/jolars/badness/compare/badness-parser-v0.2.0...badness-parser-v0.3.0) (2026-08-20)

### Breaking changes
- **parser:** pair one-sided environment aliases ([`d757cdc`](https://github.com/jolars/badness/commit/d757cdca2dd65809ebd7f08f0e3e288e5036c45c)), closes [#117](https://github.com/jolars/badness/issues/117)
- **parser:** arity-directed expl3 attachment (#119) ([`5f2f9d8`](https://github.com/jolars/badness/commit/5f2f9d8d7d1abd93b054616e34ed07aa121da662))

### Features
- **parser:** intra-file incremental reparse (#130) ([`393e0c3`](https://github.com/jolars/badness/commit/393e0c3e85b10226e58afb7b37f78cfe535dd9fd))
- **parser:** pair one-sided environment aliases ([`d757cdc`](https://github.com/jolars/badness/commit/d757cdca2dd65809ebd7f08f0e3e288e5036c45c)), closes [#117](https://github.com/jolars/badness/issues/117)
- **parser:** arity-directed expl3 attachment (#119) ([`5f2f9d8`](https://github.com/jolars/badness/commit/5f2f9d8d7d1abd93b054616e34ed07aa121da662))
- **formatter:** wrap picture statements at TikZ unit boundaries ([`5079f96`](https://github.com/jolars/badness/commit/5079f9607c5c8fcc9d021ba5b270347f7c6af524))
- **formatter:** hang statement continuations in picture bodies ([`5266aba`](https://github.com/jolars/badness/commit/5266aba65c4f0caa2cf5deebe8925ef70c9b9689))
- **parser:** wrap picture-body statements in STATEMENT nodes ([`abb16f4`](https://github.com/jolars/badness/commit/abb16f4948aea7e275cdba622ab4571701cd7682))

### Bug Fixes
- **parser:** isolate command definition names ([`8d6e274`](https://github.com/jolars/badness/commit/8d6e274957da4781668e25d9f2cd8261ee87cd34)), fixes [#133](https://github.com/jolars/badness/issues/133)
- preserve dtx documentation math ([`de0c54c`](https://github.com/jolars/badness/commit/de0c54cbedb2ae7cea6ab55204278fdd5bf2a49f)), fixes [#138](https://github.com/jolars/badness/issues/138)
- add ControlSequence ([`c2fb13d`](https://github.com/jolars/badness/commit/c2fb13d1190ff9aa7da5eca94e531059a751da21))
- **parser:** preserve def dollar delimiters ([`2381bc5`](https://github.com/jolars/badness/commit/2381bc56a3ff38ee9284e50ce33a962fc5881eae)), fixes [#129](https://github.com/jolars/badness/issues/129)

## [0.2.0](https://github.com/jolars/badness/compare/badness-parser-v0.1.1...badness-parser-v0.2.0) (2026-08-14)

### Features
- **config:** declare environments in `badness.toml` (#115) ([`a80b5af`](https://github.com/jolars/badness/commit/a80b5af3a42d8587cd323eb7f3e7bbdb4e20da5b))
- **formatter:** lay out picture bodies as statements ([`b437091`](https://github.com/jolars/badness/commit/b4370913f7bda378a41ca5b10c4ab01eb1cee35c)), closes [#114](https://github.com/jolars/badness/issues/114)
- **linter:** add `% badness-lint` suppression directives ([`c03114d`](https://github.com/jolars/badness/commit/c03114d711e15157c22d7fc98c928e13da5f1285)), refs [#114](https://github.com/jolars/badness/issues/114)
- **formatter:** add suppression comment directives ([`1810cde`](https://github.com/jolars/badness/commit/1810cdedaf0e8290a0ab04e28ba0f4360f4f282e)), refs [#114](https://github.com/jolars/badness/issues/114)
- **formatter:** segment a mandatory keyval group ([`d79ec73`](https://github.com/jolars/badness/commit/d79ec73a7cef2317880cd33b2eac98725cd05f8a))
- **semantic:** add curated block-level command property ([`c54a5ff`](https://github.com/jolars/badness/commit/c54a5ffce69c459a39415a23ab210b5530fcdd37))
- **parser:** pair user-defined environment delimiters ([`2bbff60`](https://github.com/jolars/badness/commit/2bbff600db873eb5c61008972924b318b7f01d4e)), closes [#109](https://github.com/jolars/badness/issues/109)
- **formatter:** lay conditionals out all-or-nothing ([`ed84bfe`](https://github.com/jolars/badness/commit/ed84bfef3441f467d40737bd12255dfcefbd6b71))
- **parser:** gated `CONDITIONAL` node for `\if…\else…\or…\fi` ([`e0ca4ef`](https://github.com/jolars/badness/commit/e0ca4ef71e149ff715d59fb13afdbd973221d622))
- **bib:** parse and preserve `%` comments ([`e005cc9`](https://github.com/jolars/badness/commit/e005cc96242ca01d886e18c2e32ccd027f8471c3))
- **formatter:** explode sibling-attached expl3 branches ([`d3fc51a`](https://github.com/jolars/badness/commit/d3fc51a9c5f7e8274e3cca8db885be1cc4831d77))
- **formatter:** expand optional arguments to the width ([`4c28ba4`](https://github.com/jolars/badness/commit/4c28ba4898ee417516acbc168b62388c3e3ba6d5))

### Bug Fixes
- **parser:** break every gate's run at a docstrip guard ([`d682e8f`](https://github.com/jolars/badness/commit/d682e8f6e648695a32f8a3502d3afb42f8b015ee))
- **parser:** harden environment-alias pairing ([`84f11a2`](https://github.com/jolars/badness/commit/84f11a2a3fbc931fbe19cf87e3d9ba125e35323d))

### Performance Improvements
- **parser:** answer `on_doc_margin_line` from a pre-scan ([`930380b`](https://github.com/jolars/badness/commit/930380b14671b5e0148af0fcfe99830e1f14a1da))
- **parser:** one batch driver for all nine shape gates (#113) ([`9e01ee5`](https://github.com/jolars/badness/commit/9e01ee557378bd11bde3f792be0b4de00ae75eca))
- **parser:** bound the environment-alias closer scan ([`ae83909`](https://github.com/jolars/badness/commit/ae83909940c2d1fba88f1e05aa05e461915f337a))

## [0.1.1](https://github.com/jolars/badness/compare/badness-parser-v0.1.0...badness-parser-v0.1.1) (2026-08-07)

### Bug Fixes
- **formatter:** accept a relation as an expl3 `N` slot ([`4a3d92b`](https://github.com/jolars/badness/commit/4a3d92b908081b34c0899f471efabe8d573e3c3e)), closes [#106](https://github.com/jolars/badness/issues/106)
- **formatter:** gate the expl3 forced-break dispatch in fallback lines ([`7437f69`](https://github.com/jolars/badness/commit/7437f692b549ef64f7450e4e63450ba09943181b))
- **linter:** skip parameter-template keys in key scans ([`928aa4a`](https://github.com/jolars/badness/commit/928aa4ace51aa193430a1445f9d7d6afacff8f2e)), closes [#104](https://github.com/jolars/badness/issues/104)
