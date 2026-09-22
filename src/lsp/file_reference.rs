//! Literal file arguments shared by navigation and file refactoring.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use rowan::{TextRange, TextSize};

use crate::ast::{AstNode, Environment, command_name, nth_group_inner};
use crate::completion::FileArgKind;
use crate::project::include::subfiles_parent_arg;
use crate::project::texmf::TexmfIndex;
use crate::syntax::{SyntaxKind, SyntaxNode};

use super::document_link::{comma_spans, resolve_existing};

#[derive(Debug, Clone)]
pub(super) struct Literal {
    pub range: TextRange,
    pub text: String,
}

impl Literal {
    fn new(range: TextRange, text: &str) -> Self {
        let trimmed = text.trim();
        let start = range.start() + TextSize::from((text.len() - text.trim_start().len()) as u32);
        Self {
            range: TextRange::new(start, start + TextSize::from(trimmed.len() as u32)),
            text: trimmed.to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct FileReference {
    pub command: String,
    pub path: Literal,
    pub directory: Option<Literal>,
    pub kind: FileArgKind,
    pub source_load: SourceLoad,
    pub in_preamble: bool,
    /// Navigation preserves its existing underline, including group whitespace.
    pub link_range: TextRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceLoad {
    None,
    Full,
    /// Standalone subfiles borrow their parent's preamble in the child's context.
    Preamble,
}

impl FileReference {
    pub fn strips_spaces(&self) -> bool {
        matches!(
            self.command.as_str(),
            "usepackage" | "RequirePackage" | "bibliography"
        )
    }

    pub fn appended_extension(&self, path: &str) -> Option<&'static str> {
        match self.kind {
            FileArgKind::Package => Some("sty"),
            FileArgKind::Class => Some("cls"),
            // Include loaders strip a trailing .tex before appending it again,
            // so other dotted names still need the source suffix.
            FileArgKind::TexSource
                if matches!(
                    self.command.as_str(),
                    "include" | "includeonly" | "subfileinclude"
                ) && !path.ends_with(".tex") =>
            {
                Some("tex")
            }
            _ => None,
        }
    }

    pub fn lookup_path<'a>(&self, path: &'a str) -> Cow<'a, str> {
        // These loaders remove spaces from names before filesystem lookup.
        // Keep the authored spelling and range intact for navigation and edits.
        let mut path = if self.strips_spaces() && path.contains(' ') {
            Cow::Owned(path.replace(' ', ""))
        } else {
            Cow::Borrowed(path)
        };
        if let Some(extension) = self.appended_extension(&path) {
            path.to_mut().push('.');
            path.to_mut().push_str(extension);
        }
        path
    }

    pub fn base(&self, source_dir: &Path) -> PathBuf {
        self.directory.as_ref().map_or_else(
            || source_dir.to_path_buf(),
            |dir| source_dir.join(&dir.text),
        )
    }

    pub fn extensions(&self) -> &'static [&'static str] {
        match self.kind {
            FileArgKind::Package => &["sty"],
            FileArgKind::Class => &["cls"],
            kind => kind.extensions(),
        }
    }

    pub fn candidates(&self, path: &str) -> Vec<PathBuf> {
        let lookup = self.lookup_path(path);
        let raw = PathBuf::from(lookup.as_ref());
        let default = match self.kind {
            FileArgKind::TexSource => Some("tex"),
            FileArgKind::Bib if self.command == "bibliography" => Some("bib"),
            _ => None,
        };
        // Kpathsea tries the default suffix before other suffixes, then the
        // literal name. A dot in the filename does not pin these loaders.
        if let Some(extension) = default {
            if raw.extension().is_some_and(|ext| ext == extension) {
                vec![raw]
            } else {
                vec![PathBuf::from(format!("{lookup}.{extension}")), raw]
            }
        } else if raw.extension().is_some() {
            vec![raw]
        } else {
            self.extensions()
                .iter()
                .map(|ext| raw.with_extension(ext))
                .collect()
        }
    }

    pub fn resolve(
        &self,
        source_dir: Option<&Path>,
        texmf: &TexmfIndex,
        navigation: bool,
    ) -> Option<PathBuf> {
        // Keep the import directory in the raw spelling: an explicit directory
        // must prevent the resolver's bare-name TEXMF fallback.
        let candidates = self
            .candidates(&self.path.text)
            .into_iter()
            .map(|path| match &self.directory {
                Some(dir) => Path::new(&dir.text).join(path),
                None => path,
            })
            .collect();
        resolve_existing(
            candidates,
            navigation && matches!(self.kind, FileArgKind::Package | FileArgKind::Class),
            source_dir,
            texmf,
        )
    }
}

pub(super) fn file_references(root: &SyntaxNode) -> Vec<FileReference> {
    let mut out = Vec::new();
    // A nested document environment may be a macro body. Only a top-level
    // document boundary proves where a borrowed preamble stops.
    let preamble_end = root
        .descendants()
        .filter_map(Environment::cast)
        .find(|env| {
            env.name().as_deref() == Some("document")
                && env.syntax().ancestors().skip(1).all(|ancestor| {
                    matches!(ancestor.kind(), SyntaxKind::ROOT | SyntaxKind::PARAGRAPH)
                })
        })
        .map_or(root.text_range().end(), |env| {
            env.syntax().text_range().start()
        });
    for command in root
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
    {
        let Some(name) = command_name(&command) else {
            continue;
        };
        let in_preamble = command.text_range().start() < preamble_end;
        if name == "documentclass"
            && let Some(arg) = subfiles_parent_arg(&command)
        {
            out.push(FileReference {
                command: name.to_string(),
                path: Literal::new(arg.range, &arg.text),
                directory: None,
                kind: FileArgKind::TexSource,
                source_load: SourceLoad::Preamble,
                in_preamble,
                link_range: arg.range,
            });
        }
        let (kind, group, list, import) = match name.as_str() {
            "input" | "include" | "subfile" | "subfileinclude" | "loadglsentries" => {
                (FileArgKind::TexSource, 0, false, false)
            }
            "includeonly" => (FileArgKind::TexSource, 0, true, false),
            "import" | "subimport" => (FileArgKind::TexSource, 1, false, true),
            "usepackage" | "RequirePackage" => (FileArgKind::Package, 0, true, false),
            "documentclass" | "LoadClass" | "LoadClassWithOptions" => {
                (FileArgKind::Class, 0, false, false)
            }
            "bibliography" => (FileArgKind::Bib, 0, true, false),
            "addbibresource" => (FileArgKind::Bib, 0, false, false),
            "includegraphics" => (FileArgKind::Graphics, 0, false, false),
            _ => continue,
        };
        let directory = if import {
            let Some((range, text)) = nth_group_inner(&command, 0) else {
                continue;
            };
            Some(Literal::new(range, &text))
        } else {
            None
        };
        let Some((range, text)) = nth_group_inner(&command, group) else {
            continue;
        };
        let paths = if list {
            comma_spans(&text, range)
        } else {
            vec![(text.as_str(), range)]
        };
        for (text, range) in paths {
            let path = Literal::new(range, text);
            if !path.text.is_empty() {
                out.push(FileReference {
                    command: name.to_string(),
                    path,
                    directory: directory.clone(),
                    kind,
                    source_load: if name != "includeonly"
                        && matches!(
                            kind,
                            FileArgKind::TexSource | FileArgKind::Package | FileArgKind::Class
                        ) {
                        SourceLoad::Full
                    } else {
                        SourceLoad::None
                    },
                    in_preamble,
                    link_range: range,
                });
            }
        }
    }
    out
}
