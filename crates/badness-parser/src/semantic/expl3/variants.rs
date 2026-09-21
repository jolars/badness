//! Literal argument-specifier compatibility for generated variants.

pub fn is_specifier(c: u8) -> bool {
    b"NVncvoxefTFpwD".contains(&c)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Conversion {
    Compatible,
    Deprecated,
    Incompatible,
}

pub fn classify(base: &[u8], variant: &[u8]) -> Conversion {
    if variant.len() > base.len() {
        return Conversion::Incompatible;
    }
    base.iter()
        .zip(variant)
        .map(|(&from, &to)| {
            if from == to || from == b'N' && to == b'c' || from == b'n' && b"oVvfex".contains(&to) {
                Conversion::Compatible
            } else if from == b'n' && b"Nc".contains(&to)
                || from == b'N' && b"noVvfex".contains(&to)
            {
                Conversion::Deprecated
            } else {
                Conversion::Incompatible
            }
        })
        .max()
        .unwrap_or(Conversion::Compatible)
}
