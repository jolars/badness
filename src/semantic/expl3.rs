//! Call-site arity for **expl3 function names**, derived from the argspec suffix —
//! the letters after the final `:` in `\cs_new:Npn`, `\tl_if_empty:nTF`, …
//!
//! Like [`xparse`](super::xparse), this is a spec mini-language that is *parsed*,
//! never executed (AGENTS.md decision #1): each letter names the **shape** an
//! argument takes at the call site, a bounded, purely lexical fact — squarely
//! decision #2's "the semantic layer assigns arity". No signature database is
//! involved: the name string alone carries the spec, so there is nothing to
//! curate and nothing to drift. Only meaningful inside an expl3 region, where
//! `:`/`_` are catcode-11 and the whole name lexes as one `CONTROL_WORD`.
//!
//! The letter-by-letter model (interface3's argument specifiers):
//!
//! - `N`, `V` → [`Expl3Slot::SingleToken`]: one token, typically a control
//!   sequence (`V` differs from `N` only in *expansion*, not call-site shape).
//! - `n`, `c`, `v`, `o`, `x`, `e`, `f` → [`Expl3Slot::Group`]: one braced
//!   `{…}` group (again, the letters differ only in how the material is
//!   processed, which we never model).
//! - `T`, `F` → [`Expl3Slot::Branch`]: a braced conditional branch. Sanctioned
//!   only as a *trailing* run — in a standard argspec `T`/`F` are always last,
//!   so a mid-spec `T`/`F` is treated as unknown.
//! - `p` → [`Expl3Slot::ParameterText`]: TeX parameter text (`#1#2…`), which
//!   has no fixed token count but a static *end*: TeX's own rule that the
//!   parameter text runs to the first explicit `{`. The consumer scans by that
//!   shape.
//! - `w` (arbitrary delimiters) and `D` (kernel primitive) have no lexically
//!   derivable call-site shape → the whole name is unrecognized (`None`), as is
//!   any unknown letter (including one added to expl3 after this list was
//!   written — new letters degrade to unrecognized, never to a wrong arity).

/// The call-site shape of one expl3 argument slot, derived from an argspec letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expl3Slot {
    /// `N`, `V`: exactly one token, typically a control sequence.
    SingleToken,
    /// `n`, `c`, `v`, `o`, `x`, `e`, `f`: one braced `{…}` group.
    Group,
    /// `T`, `F`: a braced conditional branch (a [`Group`](Expl3Slot::Group) a
    /// consumer may lay out specially).
    Branch,
    /// `p`: TeX parameter text — the tokens up to (not including) the next
    /// explicit `{`.
    ParameterText,
}

/// The argument slots of an expl3 function name, read from its argspec suffix
/// (the substring after the *final* `:`), or `None` when the name has no
/// derivable call-site arity.
///
/// `Some` iff the name contains a `:` and every suffix letter is a fixed-shape
/// letter per the module docs; an empty suffix (`\scan_stop:`, `\group_end:`)
/// is `Some(vec![])` — a recognized zero-argument call. `None` for a colonless
/// name (`\def`, `\@ifpackageloaded`), or a spec containing `w`, `D`, a
/// mid-spec `T`/`F`, or any unknown letter.
pub fn expl3_slots(name: &str) -> Option<Vec<Expl3Slot>> {
    let argspec = name.rsplit_once(':')?.1;
    let chars: Vec<char> = argspec.chars().collect();
    let branches = chars
        .iter()
        .rev()
        .take_while(|c| matches!(c, 'T' | 'F'))
        .count();
    let mut slots = Vec::with_capacity(chars.len());
    for c in &chars[..chars.len() - branches] {
        // `T`/`F` never match here, so a *mid*-spec `T`/`F` (nonstandard) falls
        // through to unknown.
        slots.push(match c {
            'N' | 'V' => Expl3Slot::SingleToken,
            'n' | 'c' | 'v' | 'o' | 'x' | 'e' | 'f' => Expl3Slot::Group,
            'p' => Expl3Slot::ParameterText,
            _ => return None,
        });
    }
    slots.extend(std::iter::repeat_n(Expl3Slot::Branch, branches));
    Some(slots)
}

/// The number of trailing `T`/`F` branch arguments of an expl3 conditional, read
/// from the command *name*'s argspec (the substring after the final `:`).
/// `\tl_if_empty:nTF` → `Some(2)`, `\bool_if:nT`/`:nF` → `Some(1)`; `None` for any
/// name without a `:`-argspec ending in `T`/`F` — a non-conditional expl3 function
/// (`\seq_new:N`), or a LaTeX2e command with no colon (`\@ifpackageloaded`). In an
/// expl3 argspec `T`/`F` denote *only* the true/false branch slots, so a trailing
/// `T`/`F` run is exactly the branch count.
///
/// Deliberately **not** derived from [`expl3_slots`]: this counts the raw
/// trailing run, so a name whose *earlier* letters make the arity unrecognized
/// (a hypothetical `:wTF` shape) still reports its branches — the conditional
/// layout keys on the branches alone and must not regress when the full arity
/// model bows out.
pub fn conditional_branches(name: &str) -> Option<usize> {
    let argspec = name.rsplit_once(':')?.1;
    let n = argspec
        .chars()
        .rev()
        .take_while(|c| *c == 'T' || *c == 'F')
        .count();
    (n > 0).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Expl3Slot::*;

    #[test]
    fn slots_read_from_name_suffix() {
        assert_eq!(
            expl3_slots("cs_new:Npn"),
            Some(vec![SingleToken, ParameterText, Group])
        );
        assert_eq!(
            expl3_slots("str_if_eq:nnTF"),
            Some(vec![Group, Group, Branch, Branch])
        );
        assert_eq!(
            expl3_slots("prop_get:NnNTF"),
            Some(vec![SingleToken, Group, SingleToken, Branch, Branch])
        );
        assert_eq!(expl3_slots("tl_set:Nn"), Some(vec![SingleToken, Group]));
        assert_eq!(
            expl3_slots("exp_args:NNo"),
            Some(vec![SingleToken, SingleToken, Group])
        );
        assert_eq!(expl3_slots("tl_set:Nv"), Some(vec![SingleToken, Group]));
        assert_eq!(expl3_slots("use:c"), Some(vec![Group]));
        assert_eq!(expl3_slots("tl_set:Nx"), Some(vec![SingleToken, Group]));
    }

    #[test]
    fn zero_argument_names_are_recognized() {
        assert_eq!(expl3_slots("scan_stop:"), Some(vec![]));
        assert_eq!(expl3_slots("group_begin:"), Some(vec![]));
        assert_eq!(expl3_slots("prg_return_true:"), Some(vec![]));
    }

    #[test]
    fn underivable_specs_are_unrecognized() {
        // `w`: arbitrary delimiters; `D`: kernel primitive of arbitrary arity.
        assert_eq!(expl3_slots("use_none_delimit_by_q_stop:w"), None);
        assert_eq!(expl3_slots("exp_after:wN"), None);
        assert_eq!(expl3_slots("tex_relax:D"), None);
        // Mid-spec `T`/`F` is nonstandard, so unknown.
        assert_eq!(expl3_slots("odd:TnF"), None);
        // Unknown letter anywhere bows out entirely — never a partial arity.
        assert_eq!(expl3_slots("odd:nZn"), None);
    }

    #[test]
    fn colonless_names_are_unrecognized() {
        assert_eq!(expl3_slots("def"), None);
        assert_eq!(expl3_slots("@ifpackageloaded"), None);
        assert_eq!(expl3_slots("IfBooleanTF"), None);
        assert_eq!(expl3_slots("l_tmpa_tl"), None);
    }

    #[test]
    fn exp_internal_drivers() {
        // The `\::n` expansion drivers: name is empty, spec is real. Their
        // runtime protocol is nothing like a call site, but the greedy shape
        // rules in the consumer keep them on the fallback path anyway; the
        // lexical read here is just the suffix.
        assert_eq!(expl3_slots("::n"), Some(vec![Group]));
        assert_eq!(expl3_slots(":::"), Some(vec![]));
    }

    #[test]
    fn conditional_branches_read_from_name_suffix() {
        // Trailing `T`/`F` run in the argspec (after the final `:`) is the branch
        // count; non-conditionals and colonless 2e names are `None`.
        assert_eq!(conditional_branches("tl_if_empty:nTF"), Some(2));
        assert_eq!(conditional_branches("bool_if:nT"), Some(1));
        assert_eq!(conditional_branches("bool_if:nF"), Some(1));
        assert_eq!(conditional_branches("str_if_eq:nnTF"), Some(2));
        assert_eq!(conditional_branches("int_compare:nNnTF"), Some(2));
        assert_eq!(conditional_branches("seq_map_inline:Nn"), None);
        assert_eq!(conditional_branches("prg_return_true:"), None);
        assert_eq!(conditional_branches("tl_new:N"), None);
        // A LaTeX2e conditional has no `:`-argspec, so it is never matched (issue
        // #94's `\@ifpackageloaded` stays on the width path).
        assert_eq!(conditional_branches("@ifpackageloaded"), None);
        assert_eq!(conditional_branches("IfBooleanTF"), None);
    }

    #[test]
    fn branches_survive_underivable_arity() {
        // The documented asymmetry: arity bows out, branch count must not.
        assert_eq!(expl3_slots("odd_if:wTF"), None);
        assert_eq!(conditional_branches("odd_if:wTF"), Some(2));
    }
}
