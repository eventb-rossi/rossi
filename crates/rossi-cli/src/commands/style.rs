//! Formatting style options shared by the commands that emit Event-B text
//! (`fmt`, `import`): the style preset and the per-axis overrides.

use clap::{Args, ValueEnum};
use rossi::{DeclListLayout, KeywordCase, PrettyPrinter, Style, StyleOverrides};

#[derive(Args)]
pub struct StyleArgs {
    /// Formatting style preset for Event-B text output
    #[arg(long, value_enum, default_value_t = StylePreset::Camille, value_name = "STYLE")]
    style: StylePreset,

    /// Override the preset's keyword case
    #[arg(long, value_enum, value_name = "CASE")]
    keyword_case: Option<KeywordCaseArg>,

    /// Override the preset's declaration-list layout
    /// (variables/sets/constants/any)
    #[arg(long, value_enum, value_name = "LAYOUT")]
    decl_lists: Option<DeclListsArg>,

    /// Override the preset's blank line between top-level clauses
    #[arg(long, value_name = "BOOL")]
    blank_between_clauses: Option<bool>,

    /// Maximum line width for Event-B text output, measured in characters
    /// (a tab counts as one); long formulas wrap onto operator-leading
    /// continuation lines. 0 disables wrapping
    #[arg(long, value_name = "N", default_value_t = rossi::DEFAULT_MAX_LINE_WIDTH)]
    max_width: usize,

    /// Emit Rodin's private-use glyphs (U+E100..E103) for `<<->`, `<->>`,
    /// `<<->>` and `<+` instead of their ASCII spelling, for tools that read
    /// only Rodin's spelling. They render only under Rodin's Brave Sans Mono
    /// font, and the flag has no effect with --ascii
    #[arg(long)]
    private_use_glyphs: bool,
}

impl StyleArgs {
    /// The printer these style options denote — the one CLI construction,
    /// shared by `fmt` and `import` so the two can never format text
    /// differently. `indent` is the `--indent` value; `None` follows the
    /// preset. Emitted text stays portable unless `--private-use-glyphs` asks
    /// for Rodin's spelling of the four relation/override operators.
    pub fn printer(&self, use_unicode: bool, indent: Option<&str>) -> PrettyPrinter {
        PrettyPrinter::resolved(
            self.style.into(),
            &StyleOverrides {
                keyword_case: self.keyword_case.map(Into::into),
                decl_lists: self.decl_lists.map(Into::into),
                blank_between_clauses: self.blank_between_clauses,
                indent: indent.map(str::to_string),
                use_unicode,
                private_use_glyphs: self.private_use_glyphs,
                max_line_width: self.max_width,
            },
        )
    }
}

/// Mirror of [`rossi::Style`] so the rossi crate stays clap-free.
#[derive(Clone, Copy, ValueEnum)]
pub enum StylePreset {
    /// Rodin Camille text-editor layout: lowercase keywords, inline
    /// declaration lists, blank line between clauses, 2-space indent
    Camille,
    /// rossi's original layout: uppercase keywords, one-per-line lists,
    /// no blank lines between clauses, 4-space indent
    Rossi,
}

impl From<StylePreset> for Style {
    fn from(preset: StylePreset) -> Self {
        match preset {
            StylePreset::Camille => Style::Camille,
            StylePreset::Rossi => Style::Rossi,
        }
    }
}

/// Mirror of [`rossi::KeywordCase`].
#[derive(Clone, Copy, ValueEnum)]
pub enum KeywordCaseArg {
    Lower,
    Upper,
}

impl From<KeywordCaseArg> for KeywordCase {
    fn from(case: KeywordCaseArg) -> Self {
        match case {
            KeywordCaseArg::Lower => KeywordCase::Lower,
            KeywordCaseArg::Upper => KeywordCase::Upper,
        }
    }
}

/// Mirror of [`rossi::DeclListLayout`].
#[derive(Clone, Copy, ValueEnum)]
pub enum DeclListsArg {
    Inline,
    OnePerLine,
}

impl From<DeclListsArg> for DeclListLayout {
    fn from(layout: DeclListsArg) -> Self {
        match layout {
            DeclListsArg::Inline => DeclListLayout::Inline,
            DeclListsArg::OnePerLine => DeclListLayout::OnePerLine,
        }
    }
}
