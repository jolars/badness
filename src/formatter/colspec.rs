//! Parsing the `{lcr}` column specification of a `tabular`/`array` environment
//! into per-column alignments. This is a pure layout concern owned by the
//! formatter (AGENTS.md tenet #1): it reads only the static, structural argument
//! text and resolves no macro meaning. It is deliberately **conservative** — any
//! token it does not model collapses the whole spec to `None`, and the caller
//! falls back to all-left rather than mis-align an exotic specification.

/// The horizontal alignment of one table column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColAlign {
    Left,
    Center,
    Right,
}

/// Parse a LaTeX column specification (the `{…}` argument of `\begin{tabular}{…}`)
/// into one [`ColAlign`] per produced column. Returns `None` when the spec contains
/// any token we do not model.
///
/// Recognized:
/// - `l`/`c`/`r` — one Left/Center/Right column each.
/// - `p{…}`/`m{…}`/`b{…}` — one Left column (a fixed-width paragraph box).
/// - `>{…}`/`<{…}`/`@{…}`/`!{…}` — inserts material between columns; produces no
///   column, its brace group is consumed.
/// - `*{n}{sub}` — repeats `sub` `n` times.
/// - `|`, `:`, and whitespace — ignored.
pub fn parse_column_spec(spec: &str) -> Option<Vec<ColAlign>> {
    let chars: Vec<char> = spec.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    parse_into(&chars, &mut i, &mut out)?;
    Some(out)
}

fn parse_into(chars: &[char], i: &mut usize, out: &mut Vec<ColAlign>) -> Option<()> {
    while *i < chars.len() {
        match chars[*i] {
            'l' => {
                out.push(ColAlign::Left);
                *i += 1;
            }
            'c' => {
                out.push(ColAlign::Center);
                *i += 1;
            }
            'r' => {
                out.push(ColAlign::Right);
                *i += 1;
            }
            // A paragraph-style column (`p{3cm}`/`m{…}`/`b{…}`) is a fixed-width box;
            // its content reads as left-aligned for our purposes.
            'p' | 'm' | 'b' => {
                *i += 1;
                group_text(chars, i)?;
                out.push(ColAlign::Left);
            }
            // Injected material (`>{…}`/`<{…}`/`@{…}`/`!{…}`): consume the group, add
            // no column.
            '>' | '<' | '@' | '!' => {
                *i += 1;
                group_text(chars, i)?;
            }
            // `*{n}{sub}`: repeat the sub-spec `n` times.
            '*' => {
                *i += 1;
                let count: usize = group_text(chars, i)?.trim().parse().ok()?;
                let sub = group_text(chars, i)?;
                let sub_chars: Vec<char> = sub.chars().collect();
                let mut expanded = Vec::new();
                let mut j = 0;
                parse_into(&sub_chars, &mut j, &mut expanded)?;
                for _ in 0..count {
                    out.extend(expanded.iter().copied());
                }
            }
            // Vertical rules and inter-column whitespace produce no column.
            '|' | ':' => *i += 1,
            c if c.is_whitespace() => *i += 1,
            _ => return None,
        }
    }
    Some(())
}

/// Consume a `{…}` group starting at (or after leading whitespace from) `*i`,
/// returning its inner text with the outer braces dropped. Balances nested braces.
/// Returns `None` when the next token is not a group or the group is unbalanced.
fn group_text(chars: &[char], i: &mut usize) -> Option<String> {
    while *i < chars.len() && chars[*i].is_whitespace() {
        *i += 1;
    }
    if *i >= chars.len() || chars[*i] != '{' {
        return None;
    }
    *i += 1; // past '{'
    let start = *i;
    let mut depth = 1usize;
    while *i < chars.len() {
        match chars[*i] {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let inner: String = chars[start..*i].iter().collect();
                    *i += 1; // past '}'
                    return Some(inner);
                }
            }
            _ => {}
        }
        *i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::ColAlign::*;
    use super::*;

    #[test]
    fn plain_lcr() {
        assert_eq!(parse_column_spec("lcr"), Some(vec![Left, Center, Right]));
    }

    #[test]
    fn vertical_rules_ignored() {
        assert_eq!(parse_column_spec("l|c||r"), Some(vec![Left, Center, Right]));
    }

    #[test]
    fn whitespace_ignored() {
        assert_eq!(parse_column_spec("l c r"), Some(vec![Left, Center, Right]));
    }

    #[test]
    fn paragraph_columns_are_left() {
        assert_eq!(
            parse_column_spec("p{3cm}cr"),
            Some(vec![Left, Center, Right])
        );
        assert_eq!(parse_column_spec("m{2cm}b{1cm}"), Some(vec![Left, Left]));
    }

    #[test]
    fn star_repeat() {
        assert_eq!(
            parse_column_spec("*{3}{c}"),
            Some(vec![Center, Center, Center])
        );
        assert_eq!(
            parse_column_spec("l*{2}{cr}"),
            Some(vec![Left, Center, Right, Center, Right])
        );
    }

    #[test]
    fn injected_material_adds_no_column() {
        assert_eq!(parse_column_spec("@{}lr@{}"), Some(vec![Left, Right]));
        assert_eq!(parse_column_spec(">{\\bfseries}l"), Some(vec![Left]));
        assert_eq!(
            parse_column_spec(">{\\centering\\arraybackslash}p{2cm}r"),
            Some(vec![Left, Right])
        );
    }

    #[test]
    fn nested_braces_balance() {
        assert_eq!(parse_column_spec(">{\\foo{a}{b}}c"), Some(vec![Center]));
    }

    #[test]
    fn unknown_token_bails() {
        assert_eq!(parse_column_spec("xyz"), None);
        assert_eq!(parse_column_spec("lqr"), None);
    }

    #[test]
    fn missing_or_unbalanced_group_bails() {
        assert_eq!(parse_column_spec("p"), None);
        assert_eq!(parse_column_spec("p{3cm"), None);
        assert_eq!(parse_column_spec("*{x}{c}"), None);
    }
}
