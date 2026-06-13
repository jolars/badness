# Changelog

## [0.3.0](https://github.com/jolars/badness/compare/v0.2.0...v0.3.0) (2026-06-13)

### Features
- **formatter:** align itemize blocks ([`47a2b19`](https://github.com/jolars/badness/commit/47a2b199525a17bb9a1f4bffde833e7d2433ed1a))
- **formatter:** don't indent document environment ([`3cd0d04`](https://github.com/jolars/badness/commit/3cd0d043e5fa2d2fa8e008986e896ebb32bd4a14))
- **linter:** add rule layer with duplicate-label and deprecated-command ([`4aaee37`](https://github.com/jolars/badness/commit/4aaee372a4bdf33b6a6edd24948cf5b721b97280))
- align & columns in align/matrix environments ([`d5abdca`](https://github.com/jolars/badness/commit/d5abdca6319aed86cca105ed5c5c06e8ccaf9d0a))
- match \left … \right delimiter pairs in math ([`3079875`](https://github.com/jolars/badness/commit/30798752941eb6956592a572572a0f6130a49002))
- add structured math model and math formatting ([`02802f6`](https://github.com/jolars/badness/commit/02802f60121ac265e6ef4b2a2b6549577b6ad62e))
- support argument-taking verbatim environments ([`ab8eb74`](https://github.com/jolars/badness/commit/ab8eb74a744d546c6dcfc09e6a3d7df11a369ca1))
- add file-walk for formatter ([`1603230`](https://github.com/jolars/badness/commit/16032307362a53e5cf8379e44c6428b0493a5e8a))

### Bug Fixes
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
