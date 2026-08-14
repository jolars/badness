# Changelog

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
