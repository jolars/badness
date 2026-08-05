//! The trivia-perturbation oracle for trivia-invariant layout.
//!
//! Layout must be a function of non-trivia content, config, and only those
//! trivia predicates the formatter itself *preserves* (`AGENTS.md`, and
//! `formatter.md` § *Trivia-invariant layout*). The unsafe predicate is
//! "gap is a lone newline vs. a space": the formatter converts freely in both
//! directions, so any layout decision keyed on it makes pass 1 silently edit
//! pass 2's input — the root of the K&R↔Allman idempotency bug family.
//!
//! This module checks the invariant directly, and strictly stronger than
//! idempotence: generate TeX-identical trivia perturbations of the input (swap
//! a lone newline for a space and back wherever the swap is meaning-preserving)
//! and assert `fmt(perturbed) == fmt(original)`. Idempotence only ever
//! exercises the single perturbation `fmt` happens to produce; this one does
//! not need a corpus file to land on exactly the right column arithmetic.
//!
//! The oracle is scoped to **Tier 1** layout: callers run it under
//! [`WrapMode::Reflow`](super::WrapMode::Reflow) (the Tier-2 modes —
//! `Stable`/`Sentence`/`Semantic`/`Preserve`, and `ReflowKind::Statement`
//! regions — are *defined* by authored breaks). Generator-side, a gap inside a
//! generic brace group is skipped for the same reason (`ReflowKind::Statement`
//! owns those bodies and carries a written fixed-point argument), while a gap
//! inside an expl3 region is *kept*: expl3 statement splitting reads the unsafe
//! predicate accidentally, and surfacing that is the oracle's job.
//!
//! This is a debug/test surface shared by `badness debug format --checks
//! trivia` and the invariant tests; it carries no stability promise.

use rowan::TextRange;

use super::core::expl3_regions;
use crate::parser::{LexConfig, parse_with_flavor};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

/// One meaning-preserving trivia perturbation of an input text.
#[derive(Debug, Clone)]
pub struct PerturbedVariant {
    /// `"all-newlines-to-spaces"`, `"all-spaces-to-newlines"`, or a localized
    /// `"flip@<byte>-<direction>"` single-gap reproducer.
    pub label: String,
    /// The perturbed input, verified to parse to the same skeleton and
    /// non-trivia content as the original.
    pub text: String,
}

/// The verified perturbations of one input.
#[derive(Debug, Clone)]
pub struct TriviaPerturbations {
    pub variants: Vec<PerturbedVariant>,
    /// How many gaps were eligible for a swap at all.
    pub eligible_gaps: usize,
    /// Variants dropped by post-hoc verification (the perturbed text parsed
    /// with errors, or its non-trivia content or CST skeleton changed). A
    /// nonzero count means a parser shape gate is newline-sensitive at one of
    /// the swapped gaps — a parser finding, not a layout finding, but worth
    /// triage on its own.
    pub dropped_unsafe: usize,
}

/// A passing oracle run.
#[derive(Debug, Clone, Copy)]
pub struct TriviaReport {
    pub variants_checked: usize,
    pub dropped_unsafe: usize,
}

/// A perturbation that formatted differently from the original — a layout
/// decision keyed on the unsafe lone-newline-vs-space predicate.
#[derive(Debug, Clone)]
pub struct TriviaFailure {
    /// The [`PerturbedVariant::label`] of the offending variant.
    pub label: String,
    /// The perturbed input — the reproducer.
    pub perturbed_input: String,
    pub formatted_original: String,
    /// The perturbed input's formatting, or `<format error: …>` when the
    /// formatter refused an input whose parse the generator verified clean.
    pub formatted_perturbed: String,
}

/// Why [`check_trivia_invariance`] did not return a report.
#[derive(Debug, Clone)]
pub enum TriviaError {
    /// The *original* input failed to format; the oracle cannot run. The
    /// message is the formatter's error.
    Original(String),
    /// A verified perturbation formatted differently.
    Violation(Box<TriviaFailure>),
}

/// Concatenated text of every non-trivia token of `text` parsed under
/// `config` — the "whitespace-only formatter" oracle's view of content
/// (comments, `.dtx` margins, and guards are trivia here; see
/// `tests/format.rs`). Comparing concatenated *text* rather than token
/// boundaries tolerates the math operator split re-grouping a catcode-12 run.
pub fn nontrivia_content(text: &str, config: impl Into<LexConfig>) -> String {
    node_nontrivia_content(&parse_with_flavor(text, config).syntax())
}

/// Generate the verified, TeX-identical trivia perturbations of `input`:
/// two bulk variants (every eligible lone newline → space; every eligible
/// single space → newline) plus up to `single_flip_samples` deterministic
/// single-gap variants. Returns no variants when `input` does not parse
/// cleanly.
pub fn trivia_perturbations(
    input: &str,
    config: impl Into<LexConfig>,
    single_flip_samples: usize,
) -> TriviaPerturbations {
    let config = config.into();
    let parsed = parse_with_flavor(input, config);
    if !parsed.errors.is_empty() {
        return TriviaPerturbations {
            variants: Vec::new(),
            eligible_gaps: 0,
            dropped_unsafe: 0,
        };
    }
    let root = parsed.syntax();
    let regions = expl3_regions(&root);
    let margined = margined_line_ranges(&root);
    let gaps = collect_gaps(&root, &regions, &margined);

    let original_content = node_nontrivia_content(&root);
    let original_skeleton = skeleton(&root);
    let mut out = TriviaPerturbations {
        variants: Vec::new(),
        eligible_gaps: gaps.len(),
        dropped_unsafe: 0,
    };

    // Post-hoc safety net: a swap is meaning-preserving by construction at the
    // TeX-token level (a lone newline and a space are the same space token),
    // but a parser *shape* gate may still read the physical line — verify the
    // perturbed text parses cleanly to the same skeleton and content, and drop
    // (counting) the variant otherwise.
    let push = |label: String, text: String, out: &mut TriviaPerturbations| {
        let parsed = parse_with_flavor(&text, config);
        if !parsed.errors.is_empty() {
            out.dropped_unsafe += 1;
            return;
        }
        let root = parsed.syntax();
        if node_nontrivia_content(&root) != original_content || skeleton(&root) != original_skeleton
        {
            out.dropped_unsafe += 1;
            return;
        }
        out.variants.push(PerturbedVariant { label, text });
    };

    for (direction, label) in [
        (Direction::NewlineToSpace, "all-newlines-to-spaces"),
        (Direction::SpaceToNewline, "all-spaces-to-newlines"),
    ] {
        let bulk: Vec<&Gap> = gaps.iter().filter(|g| g.direction == direction).collect();
        if !bulk.is_empty() {
            push(label.to_string(), splice(input, &bulk), &mut out);
        }
    }

    // Deterministic single-flip samples: localized reproducers for triage. The
    // LCG (Numerical Recipes constants, as in the stable-wrap fuzz test) is
    // seeded from an FNV-1a hash of the content, so runs are reproducible
    // across platforms without a PRNG dependency.
    let mut rng = Lcg(fnv1a(input));
    let mut picked: Vec<usize> = Vec::new();
    if gaps.len() <= single_flip_samples {
        picked.extend(0..gaps.len());
    } else {
        while picked.len() < single_flip_samples {
            let i = rng.below(gaps.len());
            if !picked.contains(&i) {
                picked.push(i);
            }
        }
        picked.sort_unstable();
    }
    for i in picked {
        let gap = &gaps[i];
        let dir = match gap.direction {
            Direction::NewlineToSpace => "nl-to-space",
            Direction::SpaceToNewline => "space-to-nl",
        };
        let label = format!("flip@{}-{dir}", u32::from(gap.range.start()));
        push(label, splice(input, &[gap]), &mut out);
    }

    out
}

/// Run the Tier-1 trivia-invariance oracle: `fmt(perturbed) == fmt(original)`
/// for every verified perturbation of `input`. The caller supplies the
/// formatting closure (tests pass `format_with_style`; the CLI passes its
/// package-aware pipeline), so the oracle loop exists exactly once. The caller
/// owes the Tier-1 scoping: `fmt` must lay out under
/// [`WrapMode::Reflow`](super::WrapMode::Reflow).
pub fn check_trivia_invariance(
    input: &str,
    config: impl Into<LexConfig>,
    single_flip_samples: usize,
    fmt: impl Fn(&str) -> Result<String, String>,
) -> Result<TriviaReport, TriviaError> {
    let perturbations = trivia_perturbations(input, config, single_flip_samples);
    let formatted_original = fmt(input).map_err(TriviaError::Original)?;
    let mut variants_checked = 0;
    for variant in &perturbations.variants {
        let formatted_perturbed = match fmt(&variant.text) {
            Ok(text) => text,
            Err(err) => format!("<format error: {err}>"),
        };
        if formatted_perturbed != formatted_original {
            return Err(TriviaError::Violation(Box::new(TriviaFailure {
                label: variant.label.clone(),
                perturbed_input: variant.text.clone(),
                formatted_original,
                formatted_perturbed,
            })));
        }
        variants_checked += 1;
    }
    Ok(TriviaReport {
        variants_checked,
        dropped_unsafe: perturbations.dropped_unsafe,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// A run spanning exactly one `NEWLINE` (plus any surrounding inline
    /// whitespace) becomes a single `" "`.
    NewlineToSpace,
    /// A single one-space `WHITESPACE` token becomes `"\n"`.
    SpaceToNewline,
}

/// An eligible inter-token gap: the byte range of a maximal
/// `WHITESPACE`/`NEWLINE` run and the one swap direction it admits.
#[derive(Debug, Clone)]
struct Gap {
    range: TextRange,
    direction: Direction,
}

/// The trivia kinds both content and skeleton comparisons ignore — the same
/// set the parser treats as trivia (`WHITESPACE`/`NEWLINE`/`COMMENT`/
/// `DOC_MARGIN`/`GUARD`).
fn is_ignored_trivia(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WHITESPACE
            | SyntaxKind::NEWLINE
            | SyntaxKind::COMMENT
            | SyntaxKind::DOC_MARGIN
            | SyntaxKind::GUARD
    )
}

/// The rewritable trivia kinds — mirrors the lowering's
/// `is_collapsible_trivia`.
fn is_collapsible(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE)
}

/// A neighbor kind that disqualifies a gap. Comment own-line-ness, `.dtx`
/// margins, and guards are *preserved* predicates layout may legitimately read,
/// so a swap next to one is not meaning-preserving for the oracle's purposes;
/// verbatim content is protected outright.
fn is_excluded_neighbor(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::COMMENT
            | SyntaxKind::DOC_MARGIN
            | SyntaxKind::GUARD
            | SyntaxKind::VERB
            | SyntaxKind::VERBATIM_BODY
    )
}

fn node_nontrivia_content(root: &SyntaxNode) -> String {
    root.descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| !is_ignored_trivia(t.kind()))
        .map(|t| t.text().to_string())
        .collect()
}

/// The preorder kinds of every node and non-trivia token — a trivia-blind
/// parse-shape fingerprint. A perturbation that changes it hit a
/// newline-sensitive parser gate and is dropped rather than reported as a
/// layout finding.
fn skeleton(root: &SyntaxNode) -> Vec<SyntaxKind> {
    root.descendants_with_tokens()
        .filter(|el| match el {
            SyntaxElement::Node(_) => true,
            SyntaxElement::Token(t) => !is_ignored_trivia(t.kind()),
        })
        .map(|el| el.kind())
        .collect()
}

/// Whether the gap opened by trivia token `token` is in the oracle's Tier-1
/// scope: inside an expl3 region (where the accidental
/// `Statements::SplitAtNewlines` read lives — the inventory's target), or
/// under no `GROUP` ancestor (generic multi-line group bodies are
/// `ReflowKind::Statement`, Tier 2 with a written fixed-point argument).
fn gap_in_scope(token: &SyntaxToken, regions: &[TextRange]) -> bool {
    if regions
        .iter()
        .any(|r| r.contains(token.text_range().start()))
    {
        return true;
    }
    token
        .parent_ancestors()
        .all(|node| node.kind() != SyntaxKind::GROUP)
}

/// The byte ranges (newline-inclusive) of every physical line carrying a
/// `.dtx` `DOC_MARGIN` or `GUARD` token. Margins and guards are recognized at
/// column 0 only, so a swap anywhere on such a line either splits it (leaving
/// an unmargined continuation the doc layer does not own) or pulls a code
/// line's content onto it — both rewrite the doc/code layering, not just a
/// gap. Empty outside the `.dtx` lexer mode.
fn margined_line_ranges(root: &SyntaxNode) -> Vec<TextRange> {
    let mut ranges = Vec::new();
    let mut line_start = rowan::TextSize::from(0);
    let mut line_has_margin = false;
    let mut cursor = root.first_token();
    while let Some(token) = cursor {
        match token.kind() {
            SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD => line_has_margin = true,
            SyntaxKind::NEWLINE => {
                let end = token.text_range().end();
                if line_has_margin {
                    ranges.push(TextRange::new(line_start, end));
                }
                line_start = end;
                line_has_margin = false;
            }
            _ => {}
        }
        cursor = token.next_token();
    }
    if line_has_margin {
        ranges.push(TextRange::new(line_start, root.text_range().end()));
    }
    ranges
}

/// Collect the eligible gaps of `root` in document order. A gap is a maximal
/// run of collapsible trivia with non-trivia neighbors on both sides (no
/// BOF/EOF runs), neither neighbor excluded, not touching a margined/guarded
/// line, in Tier-1 scope, and admitting exactly one swap direction: a
/// lone-newline run (blank lines are `\par` — never touched) or a lone
/// single-space token (multi-space runs and tabs are not TeX-identical to a
/// newline).
fn collect_gaps(root: &SyntaxNode, regions: &[TextRange], margined: &[TextRange]) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let mut cursor = root.first_token();
    while let Some(token) = cursor {
        if !is_collapsible(token.kind()) {
            cursor = token.next_token();
            continue;
        }
        let prev = token.prev_token();
        let start = token.text_range().start();
        let mut end = token.text_range().end();
        let mut newlines = usize::from(token.kind() == SyntaxKind::NEWLINE);
        let mut run_len = 1;
        let single_space = token.kind() == SyntaxKind::WHITESPACE && token.text() == " ";
        let mut last = token.clone();
        while let Some(next) = last.next_token().filter(|t| is_collapsible(t.kind())) {
            newlines += usize::from(next.kind() == SyntaxKind::NEWLINE);
            end = next.text_range().end();
            run_len += 1;
            last = next;
        }
        let next = last.next_token();
        let eligible = match (&prev, &next) {
            (Some(p), Some(n)) => {
                !is_excluded_neighbor(p.kind())
                    && !is_excluded_neighbor(n.kind())
                    && !margined
                        .iter()
                        .any(|r| r.contains(start) || r.contains(end))
                    && gap_in_scope(&token, regions)
            }
            _ => false,
        };
        if eligible {
            let range = TextRange::new(start, end);
            if newlines == 1 {
                gaps.push(Gap {
                    range,
                    direction: Direction::NewlineToSpace,
                });
            } else if newlines == 0 && run_len == 1 && single_space {
                gaps.push(Gap {
                    range,
                    direction: Direction::SpaceToNewline,
                });
            }
        }
        cursor = next;
    }
    gaps
}

/// Apply `gaps` (ascending, disjoint) to `input`, replacing each gap's whole
/// range with the swap target. Replacing the *entire* run folds trailing
/// spaces and the next line's indentation into the one swapped character, so
/// a swap never creates or destroys a glued junction.
fn splice(input: &str, gaps: &[&Gap]) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pos = 0usize;
    for gap in gaps {
        let (start, end) = (
            u32::from(gap.range.start()) as usize,
            u32::from(gap.range.end()) as usize,
        );
        out.push_str(&input[pos..start]);
        out.push(match gap.direction {
            Direction::NewlineToSpace => ' ',
            Direction::SpaceToNewline => '\n',
        });
        pos = end;
    }
    out.push_str(&input[pos..]);
    out
}

/// The deterministic LCG shared with the stable-wrap fuzz test (Numerical
/// Recipes constants).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 16
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() as usize) % bound.max(1)
    }
}

/// FNV-1a over the input bytes — a stable, platform-independent seed.
fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formatter::{FormatStyle, format_with_style};
    use crate::parser::LatexFlavor;

    fn perturb(input: &str) -> TriviaPerturbations {
        trivia_perturbations(input, LatexFlavor::Document, 8)
    }

    #[test]
    fn top_level_space_gap_is_eligible() {
        let p = perturb("alpha beta\n");
        assert_eq!(p.eligible_gaps, 1);
        assert_eq!(p.dropped_unsafe, 0);
        assert!(
            p.variants
                .iter()
                .any(|v| v.text == "alpha\nbeta\n" && v.label == "all-spaces-to-newlines"),
            "expected the space -> newline bulk variant, got {:?}",
            p.variants
        );
    }

    #[test]
    fn lone_newline_run_folds_to_one_space() {
        let p = perturb("a  \n  b\n");
        assert!(
            p.variants
                .iter()
                .any(|v| v.text == "a b\n" && v.label == "all-newlines-to-spaces"),
            "expected the whole run spliced to one space, got {:?}",
            p.variants
        );
    }

    #[test]
    fn blank_line_is_never_touched() {
        let p = perturb("a\n\nb\n");
        assert_eq!(p.eligible_gaps, 0);
        assert!(p.variants.is_empty());
    }

    #[test]
    fn multi_space_gap_is_ineligible() {
        let p = perturb("a  b\n");
        assert_eq!(p.eligible_gaps, 0);
    }

    #[test]
    fn comment_adjacent_gaps_are_excluded() {
        let p = perturb("a\n% note\nb\n");
        assert_eq!(p.eligible_gaps, 0);
    }

    #[test]
    fn generic_group_interior_is_excluded() {
        let p = perturb("x {a b} y\n");
        // The two gaps around the group are eligible; the one inside is not.
        assert_eq!(p.eligible_gaps, 2);
        for v in &p.variants {
            assert!(
                v.text.contains("{a b}"),
                "variant {} touched a group-interior gap: {:?}",
                v.label,
                v.text
            );
        }
    }

    #[test]
    fn expl3_region_group_interior_is_eligible() {
        let outside = perturb("x {a b} y\n");
        let inside = perturb("\\ExplSyntaxOn\nx {a b} y\n\\ExplSyntaxOff\n");
        // The expl3 region lifts the group-interior exclusion, so the region
        // holds strictly more eligible gaps than the same text outside one.
        assert!(
            inside.eligible_gaps > outside.eligible_gaps,
            "expl3 region should widen scope: {} vs {}",
            inside.eligible_gaps,
            outside.eligible_gaps
        );
    }

    #[test]
    fn dtx_margin_lines_are_excluded() {
        let p = trivia_perturbations(
            "% \\DescribeMacro{\\foo}\n% doc prose here\n",
            LexConfig {
                flavor: LatexFlavor::Package,
                dtx: true,
            },
            8,
        );
        assert_eq!(p.eligible_gaps, 0);
    }

    #[test]
    fn generation_is_deterministic() {
        let input = "alpha beta\ngamma delta epsilon\nzeta {eta theta} iota\n";
        let a = perturb(input);
        let b = perturb(input);
        let key = |p: &TriviaPerturbations| {
            p.variants
                .iter()
                .map(|v| (v.label.clone(), v.text.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(key(&a), key(&b));
    }

    #[test]
    fn unparseable_input_yields_no_variants() {
        let p = perturb("\\begin{itemize}\n");
        assert!(p.variants.is_empty());
        assert_eq!(p.eligible_gaps, 0);
    }

    #[test]
    fn oracle_passes_on_reflowed_prose() {
        let report =
            check_trivia_invariance("alpha\nbeta gamma\n", LatexFlavor::Document, 8, |s| {
                format_with_style(s, FormatStyle::default()).map_err(|e| e.to_string())
            })
            .expect("prose reflow is trivia-invariant");
        assert!(report.variants_checked > 0);
    }

    #[test]
    fn oracle_catches_the_expl3_statement_split() {
        // Two expl3 statements: the authored newline between them is the
        // statement boundary (`Statements::SplitAtNewlines`), the known
        // accidental violation. Swapping it for a space must change the
        // layout, and the oracle must say so.
        let input =
            "\\ExplSyntaxOn\n\\tl_new:N \\l_tmpa_tl\n\\tl_new:N \\l_tmpb_tl\n\\ExplSyntaxOff\n";
        let result = check_trivia_invariance(input, LatexFlavor::Document, 8, |s| {
            format_with_style(s, FormatStyle::default()).map_err(|e| e.to_string())
        });
        match result {
            Err(TriviaError::Violation(failure)) => {
                assert_ne!(failure.formatted_original, failure.formatted_perturbed);
            }
            other => panic!("expected a trivia violation, got {other:?}"),
        }
    }
}
