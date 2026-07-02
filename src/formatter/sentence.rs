//! Sentence-boundary segmentation for the [`Sentence`](super::WrapMode::Sentence)
//! and [`Semantic`](super::WrapMode::Semantic) wrap modes.
//!
//! Ported from the sibling **panache** formatter
//! (`crates/panache-formatter/src/formatter/sentence_wrap.rs`). The core is a
//! small, CST-agnostic rule engine over whitespace-split word tokens,
//! parameterized by a per-language [`LanguageProfile`]. It never runs a TeX
//! engine and reads no macro meaning — it inspects only the trailing punctuation
//! of an atom's *text*, so it stays a pure formatter concern (`AGENTS.md` tenet
//! #1: the formatter is the sole authority on layout).
//!
//! Document-language auto-detection (babel/polyglossia) is deliberately **not**
//! implemented yet; the language is chosen from config only
//! ([`sentence_language_for`]).

use std::collections::BTreeMap;

/// A built-in language whose abbreviation/starter tables drive sentence
/// segmentation. Unknown or absent languages fall back to [`Self::English`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum SentenceLanguage {
    #[default]
    English,
    Czech,
    German,
    Spanish,
    French,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundaryDecision {
    Break,
    NoBreak,
    Undecided,
}

struct BoundaryContext<'a> {
    current_word: &'a str,
    next_word: Option<&'a str>,
    has_whitespace_after: bool,
    is_last: bool,
}

struct LanguageProfile {
    no_break_abbreviations: &'static [&'static str],
    contextual_abbreviations: &'static [&'static str],
    meridiem_abbreviations: &'static [&'static str],
    sentence_starters: &'static [&'static str],
}

const ENGLISH_PROFILE: LanguageProfile = LanguageProfile {
    no_break_abbreviations: &[
        "e.g.", "i.e.", "etc.", "mr.", "mrs.", "ms.", "dr.", "prof.", "vs.", "cf.", "fig.",
        "figs.", "eq.", "dept.", "st.",
    ],
    contextual_abbreviations: &["co.", "inc.", "ltd.", "corp.", "u.s.", "u.k."],
    meridiem_abbreviations: &["a.m.", "p.m."],
    sentence_starters: &[
        "a", "an", "and", "but", "for", "he", "how", "however", "i", "in", "it", "my", "she", "so",
        "that", "the", "there", "they", "this", "we", "what", "when", "where", "who", "why", "you",
    ],
};

const CZECH_PROFILE: LanguageProfile = LanguageProfile {
    no_break_abbreviations: &[
        "např.", "tzv.", "tj.", "atd.", "apod.", "resp.", "mj.", "aj.",
    ],
    contextual_abbreviations: &[],
    meridiem_abbreviations: &[],
    sentence_starters: &[],
};

const GERMAN_PROFILE: LanguageProfile = LanguageProfile {
    no_break_abbreviations: &["bzw.", "usw.", "vgl.", "ggf."],
    contextual_abbreviations: &[],
    meridiem_abbreviations: &[],
    sentence_starters: &[],
};

// Conservative starter list; review/extend the contents as real usage surfaces
// false splits. Entries must be lowercase (candidates are lowercased before the
// comparison).
const SPANISH_PROFILE: LanguageProfile = LanguageProfile {
    no_break_abbreviations: &[
        "etc.", "p.ej.", "ej.", "vs.", "cf.", "núm.", "pág.", "págs.", "art.", "cap.", "fig.",
    ],
    contextual_abbreviations: &[],
    meridiem_abbreviations: &[],
    sentence_starters: &[],
};

// Conservative starter list; review/extend as above.
const FRENCH_PROFILE: LanguageProfile = LanguageProfile {
    no_break_abbreviations: &[
        "etc.", "cf.", "p.ex.", "ex.", "réf.", "fig.", "chap.", "éd.", "vol.",
    ],
    contextual_abbreviations: &[],
    meridiem_abbreviations: &[],
    sentence_starters: &[],
};

impl SentenceLanguage {
    fn profile(self) -> &'static LanguageProfile {
        match self {
            SentenceLanguage::English => &ENGLISH_PROFILE,
            SentenceLanguage::Czech => &CZECH_PROFILE,
            SentenceLanguage::German => &GERMAN_PROFILE,
            SentenceLanguage::Spanish => &SPANISH_PROFILE,
            SentenceLanguage::French => &FRENCH_PROFILE,
        }
    }
}

/// A built-in language profile plus any user-supplied no-break abbreviations
/// resolved for the current document. It holds two references, so it is `Copy`
/// and threads through the lowering exactly like the bare [`super::WrapMode`].
#[derive(Clone, Copy)]
pub(crate) struct ResolvedProfile<'a> {
    builtin: &'static LanguageProfile,
    /// User additions, already candidate-normalized (see
    /// [`normalize_abbreviation_candidate`]).
    extra_no_break: &'a [String],
}

impl ResolvedProfile<'static> {
    /// Built-in profile only, no user additions. Used by the unit tests.
    #[cfg(test)]
    pub(crate) fn builtin_only(language: SentenceLanguage) -> Self {
        Self {
            builtin: language.profile(),
            extra_no_break: &[],
        }
    }
}

/// The language configuration threaded into a formatting run for the
/// sentence/semantic wrap modes: a resolved built-in language plus the user's
/// merged, normalized no-break abbreviations. `Copy` (it borrows the abbreviation
/// slice), so it rides [`super::context::FormatContext`] without disturbing
/// [`super::FormatStyle`]'s `Copy`-ness. Build one with [`SentenceOptions::resolve`]
/// (from a config), [`resolve_owned`] + [`SentenceOptions::from_resolved`] (the LSP,
/// which stores owned parts on a worker job), or [`SentenceOptions::from_lang`]
/// (tests / no user extras).
#[derive(Clone, Copy, Debug)]
pub struct SentenceOptions<'a> {
    lang: SentenceLanguage,
    extra_no_break: &'a [String],
}

impl Default for SentenceOptions<'static> {
    /// English, no user abbreviations — the profile is only consulted in
    /// sentence/semantic mode, so this is a harmless default for every other run.
    fn default() -> Self {
        Self {
            lang: SentenceLanguage::English,
            extra_no_break: &[],
        }
    }
}

impl SentenceOptions<'static> {
    /// Options for a language code (e.g. `"de"`, `"en-GB"`) with no user
    /// abbreviations. The public constructor tests use to exercise a specific
    /// built-in profile without a full config.
    pub fn from_lang(lang: Option<&str>) -> Self {
        let normalized = lang.map(str::to_lowercase);
        Self {
            lang: sentence_language_for(normalized.as_deref()),
            extra_no_break: &[],
        }
    }
}

impl<'a> SentenceOptions<'a> {
    /// Resolve options from a config's `lang` string and `no-break-abbreviations`
    /// map. `scratch` owns the merged, normalized user entries for the lifetime of
    /// the returned options (the caller-owned-arena pattern panache uses in
    /// `resolve_profile`), so the caller keeps it alive across the format call(s).
    pub fn resolve(
        lang: Option<&str>,
        no_break: &BTreeMap<String, Vec<String>>,
        scratch: &'a mut Vec<String>,
    ) -> Self {
        let normalized = lang.map(str::to_lowercase);
        let language = sentence_language_for(normalized.as_deref());
        scratch.clear();
        scratch.extend(merge_no_break_list(no_break, normalized.as_deref()));
        Self {
            lang: language,
            extra_no_break: scratch.as_slice(),
        }
    }

    /// Build options from already-resolved parts (see [`resolve_owned`]). The LSP
    /// resolves the language and merged no-break list once into owned, `Send` values
    /// it stores on a worker job, then borrows them here at format time.
    pub(crate) fn from_resolved(lang: SentenceLanguage, extra_no_break: &'a [String]) -> Self {
        Self {
            lang,
            extra_no_break,
        }
    }

    pub(crate) fn resolved(self) -> ResolvedProfile<'a> {
        ResolvedProfile {
            builtin: self.lang.profile(),
            extra_no_break: self.extra_no_break,
        }
    }
}

/// Resolve a config's `lang` + `no-break-abbreviations` into owned parts: the
/// built-in language and the merged, normalized no-break list. For callers that
/// must hold the result across threads before building a borrowed
/// [`SentenceOptions`] with [`SentenceOptions::from_resolved`].
pub(crate) fn resolve_owned(
    lang: Option<&str>,
    no_break: &BTreeMap<String, Vec<String>>,
) -> (SentenceLanguage, Vec<String>) {
    let normalized = lang.map(str::to_lowercase);
    (
        sentence_language_for(normalized.as_deref()),
        merge_no_break_list(no_break, normalized.as_deref()),
    )
}

fn trim_sentence_closing_punctuation(word: &str) -> &str {
    word.trim_end_matches(['"', '\'', ')', ']', '}', '`'])
}

fn normalize_abbreviation_candidate(word: &str) -> String {
    let trimmed = trim_sentence_closing_punctuation(word)
        .trim_start_matches(['"', '\'', '(', '[', '{', '`'])
        .trim_end_matches([',', ';', ':']);
    trimmed.to_lowercase()
}

fn is_no_break_abbreviation(word: &str, profile: ResolvedProfile<'_>) -> bool {
    let candidate = normalize_abbreviation_candidate(word);
    if profile
        .builtin
        .no_break_abbreviations
        .contains(&candidate.as_str())
    {
        return true;
    }
    if profile
        .extra_no_break
        .iter()
        .any(|entry| entry == &candidate)
    {
        return true;
    }
    candidate.ends_with('.') && candidate.matches('.').count() >= 2 && {
        let without_periods = candidate.replace('.', "");
        !without_periods.is_empty() && without_periods.chars().all(|c| c.is_ascii_lowercase())
    }
}

fn normalize_next_token_candidate(word: &str) -> String {
    word.trim_start_matches(['"', '\'', '(', '[', '{', '`'])
        .trim_end_matches(['"', '\'', ')', ']', '}', ',', ';', ':', '`'])
        .to_ascii_lowercase()
}

fn starts_with_uppercase_after_opening_punct(word: &str) -> bool {
    word.trim_start_matches(['"', '\'', '(', '[', '{', '`'])
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
}

fn is_contextual_sentence_boundary(
    current_word: &str,
    next_word: Option<&str>,
    profile: &LanguageProfile,
) -> bool {
    let current = normalize_abbreviation_candidate(current_word);
    if !profile.contextual_abbreviations.contains(&current.as_str())
        && !profile.meridiem_abbreviations.contains(&current.as_str())
    {
        return false;
    }
    let Some(next) = next_word else {
        return false;
    };
    if !starts_with_uppercase_after_opening_punct(next) {
        return false;
    }
    if profile.meridiem_abbreviations.contains(&current.as_str()) {
        return true;
    }
    let next_norm = normalize_next_token_candidate(next);
    profile.sentence_starters.contains(&next_norm.as_str())
}

fn rule_ellipsis_no_break(ctx: &BoundaryContext<'_>) -> BoundaryDecision {
    let trimmed = trim_sentence_closing_punctuation(ctx.current_word);
    if trimmed.ends_with("...") || trimmed.ends_with("…") {
        return BoundaryDecision::NoBreak;
    }
    BoundaryDecision::Undecided
}

fn rule_contextual_abbreviation_break(
    ctx: &BoundaryContext<'_>,
    profile: ResolvedProfile<'_>,
) -> BoundaryDecision {
    let trimmed = trim_sentence_closing_punctuation(ctx.current_word);
    let Some(last_char) = trimmed.chars().last() else {
        return BoundaryDecision::NoBreak;
    };
    if last_char == '.' && is_contextual_sentence_boundary(trimmed, ctx.next_word, profile.builtin)
    {
        return BoundaryDecision::Break;
    }
    BoundaryDecision::Undecided
}

fn rule_abbreviation_no_break(
    ctx: &BoundaryContext<'_>,
    profile: ResolvedProfile<'_>,
) -> BoundaryDecision {
    let trimmed = trim_sentence_closing_punctuation(ctx.current_word);
    let Some(last_char) = trimmed.chars().last() else {
        return BoundaryDecision::NoBreak;
    };
    if last_char == '.' && is_no_break_abbreviation(trimmed, profile) {
        return BoundaryDecision::NoBreak;
    }
    BoundaryDecision::Undecided
}

fn rule_terminal_punctuation_break(ctx: &BoundaryContext<'_>) -> BoundaryDecision {
    let trimmed = trim_sentence_closing_punctuation(ctx.current_word);
    let Some(last_char) = trimmed.chars().last() else {
        return BoundaryDecision::NoBreak;
    };
    if matches!(last_char, '.' | '!' | '?') && (ctx.has_whitespace_after || ctx.is_last) {
        return BoundaryDecision::Break;
    }
    BoundaryDecision::NoBreak
}

/// Decide whether a sentence boundary falls *after* `word`. `next_word` is the
/// following atom's text (for the contextual-abbreviation next-token signal),
/// `has_whitespace_after` whether a space/newline separates them, and `is_last`
/// whether `word` is the final atom of its line. Runs four rules in priority
/// order (ellipsis, contextual-abbreviation break, abbreviation no-break, terminal
/// punctuation), returning the first decisive one.
pub(crate) fn decide_sentence_boundary(
    word: &str,
    next_word: Option<&str>,
    has_whitespace_after: bool,
    is_last: bool,
    profile: ResolvedProfile<'_>,
) -> BoundaryDecision {
    let ctx = BoundaryContext {
        current_word: word,
        next_word,
        has_whitespace_after,
        is_last,
    };

    let rules: [BoundaryDecision; 4] = [
        rule_ellipsis_no_break(&ctx),
        rule_contextual_abbreviation_break(&ctx, profile),
        rule_abbreviation_no_break(&ctx, profile),
        rule_terminal_punctuation_break(&ctx),
    ];

    for decision in rules {
        if decision != BoundaryDecision::Undecided {
            return decision;
        }
    }
    BoundaryDecision::NoBreak
}

pub(crate) fn is_sentence_boundary_text(
    word: &str,
    next_word: Option<&str>,
    has_whitespace_after: bool,
    is_last: bool,
    profile: ResolvedProfile<'_>,
) -> bool {
    matches!(
        decide_sentence_boundary(word, next_word, has_whitespace_after, is_last, profile),
        BoundaryDecision::Break
    )
}

/// Primary language subtag, e.g. `en-gb` -> `en`, `pt_br` -> `pt`.
fn primary_subtag(lang: &str) -> &str {
    lang.split(['-', '_']).next().unwrap_or(lang)
}

/// Map a (lowercased) language code to its built-in profile. `en`, unknown
/// languages, and absent metadata all fall back to English.
pub(crate) fn sentence_language_for(lang: Option<&str>) -> SentenceLanguage {
    match lang.map(primary_subtag) {
        Some("cs") => SentenceLanguage::Czech,
        Some("de") => SentenceLanguage::German,
        Some("es") => SentenceLanguage::Spanish,
        Some("fr") => SentenceLanguage::French,
        _ => SentenceLanguage::English,
    }
}

/// Merge the user-configured no-break abbreviations that apply to `lang`: the
/// `default` bucket plus the bucket for the language's primary subtag, each
/// normalized to a comparison candidate. `lang` is expected already lowercased.
fn merge_no_break_list(
    no_break: &BTreeMap<String, Vec<String>>,
    lang: Option<&str>,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(entries) = no_break.get("default") {
        out.extend(
            entries
                .iter()
                .map(|entry| normalize_abbreviation_candidate(entry)),
        );
    }
    if let Some(code) = lang.map(primary_subtag)
        && let Some(entries) = no_break.get(code)
    {
        out.extend(
            entries
                .iter()
                .map(|entry| normalize_abbreviation_candidate(entry)),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn english() -> ResolvedProfile<'static> {
        ResolvedProfile::builtin_only(SentenceLanguage::English)
    }

    #[test]
    fn abbreviation_periods_are_not_sentence_boundaries() {
        assert!(!is_sentence_boundary_text(
            "(e.g.)",
            Some("Next"),
            true,
            false,
            english()
        ));
        assert!(!is_sentence_boundary_text(
            "i.e.",
            Some("Next"),
            true,
            false,
            english()
        ));
        assert!(!is_sentence_boundary_text(
            "`etc.`",
            Some("Next"),
            true,
            false,
            english()
        ));
        assert!(is_sentence_boundary_text(
            "complete.",
            Some("Next"),
            true,
            false,
            english()
        ));
        assert_eq!(
            decide_sentence_boundary("complete.", Some("Next"), true, false, english()),
            BoundaryDecision::Break
        );
    }

    #[test]
    fn boundary_decision_reports_no_break_for_abbreviation() {
        assert_eq!(
            decide_sentence_boundary("e.g.", Some("Next"), true, false, english()),
            BoundaryDecision::NoBreak
        );
    }

    #[test]
    fn contextual_abbreviations_use_next_token_signal() {
        assert!(!is_sentence_boundary_text(
            "co.",
            Some("at"),
            false,
            false,
            english()
        ));
        assert!(is_sentence_boundary_text(
            "co.",
            Some("They"),
            true,
            false,
            english()
        ));
        assert!(!is_sentence_boundary_text(
            "U.S.",
            Some("Government"),
            false,
            false,
            english()
        ));
        assert!(is_sentence_boundary_text(
            "U.S.",
            Some("How"),
            true,
            false,
            english()
        ));
        assert!(!is_sentence_boundary_text(
            "p.m.",
            Some("traveler"),
            false,
            false,
            english()
        ));
    }

    #[test]
    fn german_builtin_abbreviation_no_break() {
        let de = ResolvedProfile::builtin_only(SentenceLanguage::German);
        assert!(!is_sentence_boundary_text(
            "bzw.",
            Some("Next"),
            true,
            false,
            de
        ));
        // The English profile doesn't know `bzw.`, so there it ends a sentence.
        assert!(is_sentence_boundary_text(
            "bzw.",
            Some("Next"),
            true,
            false,
            english()
        ));
    }

    #[test]
    fn czech_builtin_abbreviation_no_break_is_case_insensitive() {
        let cs = ResolvedProfile::builtin_only(SentenceLanguage::Czech);
        assert!(!is_sentence_boundary_text(
            "např.",
            Some("Next"),
            true,
            false,
            cs
        ));
        // Mixed case exercises the `to_lowercase()` normalization path.
        assert!(!is_sentence_boundary_text(
            "Např.",
            Some("Next"),
            true,
            false,
            cs
        ));
        assert!(!is_sentence_boundary_text(
            "atd.",
            Some("Next"),
            true,
            false,
            cs
        ));
    }

    #[test]
    fn spanish_non_ascii_abbreviation_matches_via_list() {
        let es = ResolvedProfile::builtin_only(SentenceLanguage::Spanish);
        // `núm.` is single-period and non-ASCII, so the multi-period heuristic
        // does not apply; it matches only because it is in the Spanish list.
        assert!(!is_sentence_boundary_text(
            "núm.",
            Some("Next"),
            true,
            false,
            es
        ));
        assert!(is_sentence_boundary_text(
            "núm.",
            Some("Next"),
            true,
            false,
            english()
        ));
        // A bogus non-list, non-ASCII multi-period token still breaks: the
        // heuristic stays ASCII-only.
        assert!(is_sentence_boundary_text(
            "ñ.ñ.",
            Some("Next"),
            true,
            false,
            english()
        ));
    }

    #[test]
    fn user_extra_abbreviations_merge_with_builtin() {
        let extras = vec!["zzz.".to_string()];
        let profile = ResolvedProfile {
            builtin: SentenceLanguage::English.profile(),
            extra_no_break: &extras,
        };
        // The user-supplied entry suppresses the break...
        assert!(!is_sentence_boundary_text(
            "zzz.",
            Some("Next"),
            true,
            false,
            profile
        ));
        // ...the built-in English entry still suppresses...
        assert!(!is_sentence_boundary_text(
            "e.g.",
            Some("Next"),
            true,
            false,
            profile
        ));
        // ...and an ordinary word still ends the sentence.
        assert!(is_sentence_boundary_text(
            "done.",
            Some("Next"),
            true,
            false,
            profile
        ));
    }

    #[test]
    fn region_subtag_selects_primary_language_profile() {
        assert!(matches!(
            sentence_language_for(Some("de-at")),
            SentenceLanguage::German
        ));
        assert!(matches!(
            sentence_language_for(Some("en-gb")),
            SentenceLanguage::English
        ));
    }

    #[test]
    fn from_lang_normalizes_case_and_region() {
        // Uppercase and a region subtag both resolve to the German profile.
        let de = SentenceOptions::from_lang(Some("DE-AT"));
        assert!(!is_sentence_boundary_text(
            "bzw.",
            Some("Next"),
            true,
            false,
            de.resolved()
        ));
    }

    #[test]
    fn resolve_sentence_options_merges_default_and_lang_buckets() {
        let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
        map.insert("default".to_string(), vec!["ibid.".to_string()]);
        map.insert("de".to_string(), vec!["Abb.".to_string()]);
        map.insert("cs".to_string(), vec!["obr.".to_string()]);

        let mut scratch = Vec::new();
        let opts = SentenceOptions::resolve(Some("de-AT"), &map, &mut scratch);
        let profile = opts.resolved();

        // The `default` bucket applies to every language...
        assert!(!is_sentence_boundary_text(
            "ibid.",
            Some("Next"),
            true,
            false,
            profile
        ));
        // ...the `de` bucket applies (normalized to lowercase `abb.`)...
        assert!(!is_sentence_boundary_text(
            "Abb.",
            Some("Next"),
            true,
            false,
            profile
        ));
        // ...but the `cs` bucket does not leak into a German document.
        assert!(is_sentence_boundary_text(
            "obr.",
            Some("Next"),
            true,
            false,
            profile
        ));
    }
}
