//! The seeded edit generator and the invariant checker, shared by both reparse
//! harnesses.
//!
//! Two callers, for two different reasons:
//!
//! - `incremental_reparse.rs` — depth. Hand-written hazard snippets plus the
//!   in-crate corpus, run as part of `cargo test`.
//! - `reparse_corpus_sweep.rs` — breadth. The pinned gate corpora, `#[ignore]`d
//!   and driven by `scripts/check_reparse_baselines.sh`.
//!
//! They share this module rather than each carrying a copy, because a divergence
//! found by one has to be reproducible in the other: the same alphabet, the same
//! generator, the same seeds, the same check. A second copy of [`HAZARD_ALPHABET`]
//! that drifted by one entry would make a sweep failure unreachable from the fast
//! suite, which is where a reduction actually gets written.
//!
//! # What is asserted
//!
//! The governing invariant, on every edit either harness generates: whenever
//! `reparse` returns `Some`, its green tree and its error vector must be
//! byte-identical to a full parse of the edited text — and the tree must still be
//! lossless, the house oracle every other parser test leans on. A `None` is
//! trivially correct, since the caller full-parses.
//!
//! The in-crate assert (`parser::reparse::assert_matches_full_parse`) already fires
//! on every successful reparse in a debug build, so this is a *generator* rather
//! than a second checker: its job is to reach shapes a hand-written test would not.
//! It re-checks anyway, because the in-crate assert is compiled out in release and
//! a `--release` run must still be worth something — and the corpus sweep runs in
//! release by necessity.
//!
//! # Why the alphabet is what it is
//!
//! Random ASCII would spend its whole budget on prose. Every entry in
//! [`HAZARD_ALPHABET`] is a character or word that changes how *later* text lexes
//! or parses — the boundary of one of the sanctioned lexer modes. Typing a `%` mid
//! line turns the rest of it into a comment; a `\begin{` opens an environment whose
//! `\end` is now missing; an `\ExplSyntaxOn` re-lexes `_` and `:` as letters for
//! the rest of the file. Those are the edits a tier has to refuse, and an alphabet
//! that cannot spell them proves nothing.
//!
//! CRLF entries are deliberate rather than incidental. Panache lost its entire
//! reparse feature on Windows-authored files to a seam predicate written as a
//! literal `"\n\n"` test — safe, since it simply never spliced, and a total loss.
//! Measuring the gap beats discovering it.

#![allow(dead_code)] // each test binary uses only part of this module.

use badness_parser::declarations::ResolvedDeclarations;
use badness_parser::parser::{
    Edit, LatexFlavor, LexConfig, Parse, ParseCtx, ReparseBase, ReparseTier, Reparsed, fingerprint,
    parse_with_declarations_resolved, reparse, reparse_edits,
};
use badness_parser::syntax::SyntaxNode;

/// A seeded linear congruential generator (MMIX constants).
///
/// This generator remains separate from proptest because the edit stream is a
/// stable test protocol. Every failure prints its seed, so a reproducer is a
/// one-line change, and the corpus sweep's recorded splice counts are a ratchet
/// only because the same seed draws the same edits across dependency versions.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next() >> 33) as usize % n
    }
}

/// Insert candidates, each chosen because it changes how later text lexes or parses.
pub const HAZARD_ALPHABET: &[&str] = &[
    // Catcode-bearing characters: the whole reason a LaTeX token tier needs a ban list.
    "\\",
    "{",
    "}",
    "$",
    "$$",
    "%",
    "&",
    "#",
    "^",
    "_",
    "~",
    "[",
    "]",
    // Environment and math delimiters.
    "\\begin{",
    "\\end{",
    "\\[",
    "\\]",
    "\\(",
    "\\)",
    "\\\\",
    // Names that route the parser: verbatim bodies, math environments, aliases.
    "verbatim",
    "lstlisting",
    "equation",
    "align",
    "itemize",
    "document",
    // Protected regions and short verbs.
    "\\verb|",
    "\\verb+",
    "\\MakeShortVerb{\\|}",
    // Letter-mode and expl3 region toggles, whose scope runs to end of file.
    "\\makeatletter",
    "\\makeatother",
    "\\ExplSyntaxOn",
    "\\ExplSyntaxOff",
    ":nn",
    ":Nn",
    "_int",
    // Conditionals, which pair by a forward scan for their closer.
    "\\iffalse",
    "\\ifnum",
    "\\else",
    "\\or",
    "\\fi",
    "\\newif",
    // `.dtx` structure: doc margins, guards, macrocode frames.
    "%",
    "%<*debug>",
    "%</debug>",
    "%    \\begin{macrocode}",
    "%    \\end{macrocode}",
    // Definition heads, which drive the second parse pass.
    "\\newcommand",
    "\\def",
    "\\let",
    "\\renewcommand",
    // Picture-body statement terminators.
    ";",
    "\\draw",
    "\\node",
    // Line structure, including the CRLF forms.
    "\n",
    "\n\n",
    "\r\n",
    "\r\n\r\n",
    " ",
    "  ",
    // Ordinary content, including a multi-byte char so offsets are exercised.
    "x",
    "1",
    "α",
    "…",
];

/// The parse inputs every case in either harness runs under.
pub fn config() -> LexConfig {
    LatexFlavor::Document.into()
}

/// One driver's running tally: how many edits it offered, how many spliced, and
/// which tier took each.
///
/// The tier breakdown is not decoration. Declining is always sound, so a change
/// that quietly demotes a workload from one tier to another leaves every assertion
/// green and only the *shape* of the tally moves — which is the thing the corpus
/// sweep's recorded baseline exists to notice.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub attempted: usize,
    pub spliced: usize,
    pub token: usize,
    pub verbatim: usize,
    pub math: usize,
    pub region: usize,
}

impl Tally {
    /// Record one attempt and its outcome.
    pub fn record(&mut self, tier: Option<ReparseTier>) {
        self.attempted += 1;
        match tier {
            None => {}
            Some(ReparseTier::Token) => {
                self.spliced += 1;
                self.token += 1;
            }
            Some(ReparseTier::Verbatim) => {
                self.spliced += 1;
                self.verbatim += 1;
            }
            Some(ReparseTier::Math) => {
                self.spliced += 1;
                self.math += 1;
            }
            Some(ReparseTier::Region) => {
                self.spliced += 1;
                self.region += 1;
            }
        }
    }

    pub fn merge(&mut self, other: &Tally) {
        self.attempted += other.attempted;
        self.spliced += other.spliced;
        self.token += other.token;
        self.verbatim += other.verbatim;
        self.math += other.math;
        self.region += other.region;
    }

    /// Splice rate, floored to a whole percent.
    pub fn percent(&self) -> usize {
        if self.attempted == 0 {
            return 0;
        }
        self.spliced * 100 / self.attempted
    }
}

/// A parsed base to splice edits against.
///
/// Owning the parse lets a driver whose base text does not move — random single
/// edits against one file — pay for it once instead of once per edit, which is
/// half the cost of the corpus sweep. A typing driver rebuilds it per keystroke,
/// which is what an editor does anyway.
pub struct Base {
    text: String,
    parse: Parse,
    ctx: ParseCtx,
    declared: ResolvedDeclarations,
    config: LexConfig,
}

impl Base {
    pub fn new(text: impl Into<String>) -> Self {
        Self::with_config(text, config())
    }

    /// A base parsed under a specific [`LexConfig`].
    ///
    /// The corpus sweep needs this: a `.sty` is loaded under an implicit
    /// `\makeatletter` and a `.dtx` under the docstrip mode, and a tier's answer
    /// differs between the regimes — the leaf tiers refuse `.dtx` outright. Sweeping
    /// package source as if it were a plain document would measure a workload no
    /// caller ever asks for.
    pub fn with_config(text: impl Into<String>, config: LexConfig) -> Self {
        let text = text.into();
        let declared = ResolvedDeclarations::default();
        let (parse, ctx) = parse_with_declarations_resolved(&text, config, &declared);
        Self {
            text,
            parse,
            ctx,
            declared,
            config,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn syntax(&self) -> SyntaxNode {
        self.parse.syntax()
    }

    fn view(&self) -> ReparseBase<'_> {
        ReparseBase::from_parts(
            &self.text,
            &self.parse.green,
            &self.parse.errors,
            &self.ctx,
            self.config,
            &self.declared,
        )
    }

    /// Check one edit against the invariant, recording the outcome in `tally`.
    ///
    /// An edit that does not fit still counts as an attempt: a driver that
    /// generated one is a driver that offered one, and hiding those would let a
    /// generator bug inflate the recorded rate.
    pub fn check(&self, edit: &Edit, seed: u64, label: &str, tally: &mut Tally) {
        if !edit.fits(&self.text) {
            tally.record(None);
            return;
        }
        let new_text = edit.apply(&self.text);
        let Some(result) = reparse(&self.view(), edit, &new_text) else {
            tally.record(None);
            return;
        };
        assert_result_matches(
            &result,
            &new_text,
            &self.declared,
            self.config,
            seed,
            label,
            edit,
        );
        tally.record(Some(result.tier));
    }

    /// The same for a chain of edits, the shape a `didChange` batch arrives in.
    pub fn check_chain(
        &self,
        chain: &[Edit],
        new_text: &str,
        seed: u64,
        label: &str,
        tally: &mut Tally,
    ) {
        let Some(result) = reparse_edits(&self.view(), chain, new_text) else {
            tally.record(None);
            return;
        };
        assert_result_matches(
            &result,
            new_text,
            &self.declared,
            self.config,
            seed,
            label,
            chain.last().expect("non-empty chain"),
        );
        tally.record(Some(result.tier));
    }
}

pub fn assert_result_matches(
    result: &Reparsed,
    new_text: &str,
    declared: &ResolvedDeclarations,
    config: LexConfig,
    seed: u64,
    label: &str,
    edit: &Edit,
) {
    let full = parse_with_declarations_resolved(new_text, config, declared).0;
    let spliced = SyntaxNode::new_root(result.green.clone());

    assert_eq!(
        fingerprint(&spliced),
        fingerprint(&full.syntax()),
        "tree diverged\n  case: {label}\n  seed: {seed}\n  tier: {:?}\n  edit: {edit:?}\n  text: {new_text:?}",
        result.tier,
    );
    assert_eq!(
        result.errors, full.errors,
        "errors diverged\n  case: {label}\n  seed: {seed}\n  tier: {:?}\n  edit: {edit:?}\n  text: {new_text:?}",
        result.tier,
    );
    // Losslessness is implied by tree equality plus the full parse's own guarantee,
    // but it is the house oracle and it costs a string compare.
    assert_eq!(
        spliced.to_string(),
        new_text,
        "losslessness failed\n  case: {label}\n  seed: {seed}\n  edit: {edit:?}",
    );
}

/// Draw one random edit against `text`: ~70% insert, ~20% delete, ~10% replace.
pub fn random_edit(rng: &mut Lcg, text: &str) -> Edit {
    let at = char_boundary_at_or_below(text, rng.below(text.len() + 1));
    match rng.below(10) {
        0..=6 => Edit {
            range: at..at,
            insert: HAZARD_ALPHABET[rng.below(HAZARD_ALPHABET.len())].to_string(),
        },
        7..=8 => {
            let end = next_char_boundary(text, at);
            Edit {
                range: at..end,
                insert: String::new(),
            }
        }
        _ => {
            let end = next_char_boundary(text, at);
            Edit {
                range: at..end,
                insert: HAZARD_ALPHABET[rng.below(HAZARD_ALPHABET.len())].to_string(),
            }
        }
    }
}

pub fn char_boundary_at_or_below(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

pub fn next_char_boundary(text: &str, at: usize) -> usize {
    let mut end = (at + 1).min(text.len());
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    end
}

/// Assert a driver still splices often enough to be testing anything.
///
/// Phrased as a percentage of attempts so an iteration-count knob does not change
/// the verdict, and reported with both numbers so a failure says how far it fell
/// rather than merely that it did. This is the tripwire panache did not have: a
/// window cutoff cost its fuzzer two thirds of its coverage while every assertion
/// it carried still passed.
pub fn assert_splice_floor(driver: &str, tally: &Tally, floor_percent: usize) {
    assert!(tally.attempted > 0, "{driver}: nothing was attempted");
    let percent = tally.percent();
    assert!(
        percent >= floor_percent,
        "{driver}: splice rate fell to {percent}% ({}/{}), floor is {floor_percent}%. \
         A guard narrowed the tier; either it is wrong or this floor moves — \
         deliberately, in its own commit.",
        tally.spliced,
        tally.attempted,
    );
}
