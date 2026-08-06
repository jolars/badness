\NeedsTeXFormat{LaTeX2e}
\ProvidesClass{demo}[2026/06/21 v1.0 demo class]

\DeclareOption{draft}{\PassOptionsToClass{\CurrentOption}{article}}
\DeclareOption*{\PassOptionsToClass{\CurrentOption}{article}}
\ProcessOptions\relax

\LoadClass{article}

\newcommand\cls@hook{%
  one two three
  four five six
}
