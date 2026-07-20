# Changelog

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
