//! Command-line driver for the Event-B grammar fuzzer.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use eventb_fuzz::choice::SplitMix64;
use eventb_fuzz::generate::{Config, Generator};

#[derive(Parser)]
#[command(
    name = "eventb-fuzz",
    version,
    about = "Grammar fuzzer for Event-B text"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Derive Event-B models from the tree-sitter grammar.
    Gen(GenArgs),
}

#[derive(clap::Args)]
struct GenArgs {
    /// How many models to derive.
    #[arg(long, default_value_t = 100)]
    count: usize,
    /// Seed for the choice stream; the same seed always derives the same
    /// models.
    #[arg(long, default_value_t = 0)]
    seed: u64,
    /// Directory to write one `.eventb` file per model into. Without it the
    /// models are derived and reported on but not kept.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Most nested rule expansions per component.
    #[arg(long, default_value_t = Config::default().max_depth)]
    max_depth: usize,
    /// Soft cap on tokens per component.
    #[arg(long, default_value_t = Config::default().max_tokens)]
    max_tokens: usize,
    /// Print the first model that Rossi's parser rejects, with its error.
    #[arg(long)]
    show_rejected: bool,
    /// Never derive this grammar rule. Repeatable. Suppressing a rule and
    /// comparing acceptance is how a class of grammar disagreement is
    /// isolated.
    #[arg(long = "suppress", value_name = "RULE")]
    suppressed: Vec<String>,
    /// Emit the Unicode spelling of every operator, which is what
    /// eventb-checker accepts. Pair with `--lowercase-keywords`.
    #[arg(long)]
    unicode: bool,
    /// Emit every structural keyword in lower case, which is what Camille
    /// accepts. Pair with `--unicode`.
    #[arg(long)]
    lowercase_keywords: bool,
    /// Write each component's sections, and each event's clauses, in the
    /// canonical order, which is the order Camille requires. Pair with the
    /// two above.
    #[arg(long)]
    ordered_clauses: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Gen(args) => generate(args),
    }
}

fn generate(args: GenArgs) -> ExitCode {
    let grammar = match eventb_fuzz::load_grammar() {
        Ok(Some(grammar)) => grammar,
        Ok(None) => {
            eprintln!("SKIP: {}", eventb_fuzz::MISSING_GRAMMAR);
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(directory) = &args.out
        && let Err(error) = std::fs::create_dir_all(directory)
    {
        eprintln!("error: {}: {error}", directory.display());
        return ExitCode::FAILURE;
    }

    let config = Config {
        max_depth: args.max_depth,
        max_tokens: args.max_tokens,
        suppressed: args.suppressed.clone(),
        unicode_operators: args.unicode,
        lowercase_keywords: args.lowercase_keywords,
        ordered_clauses: args.ordered_clauses,
        ..Config::default()
    };
    let generator = Generator::new(&grammar, config);
    let mut source = SplitMix64::new(args.seed);

    let mut accepted = 0usize;
    let mut rejections: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut shown = false;

    for index in 0..args.count {
        let model = generator.generate(&mut source);
        if !model.unsupported.is_empty() {
            eprintln!(
                "error: grammar constructs the generator cannot derive: {:?}",
                model.unsupported
            );
            return ExitCode::FAILURE;
        }
        // Written before parsing, not after: a parser crash takes the
        // process with it, and the input that caused it is the whole point.
        if let Some(directory) = &args.out {
            let file = directory.join(format!("gen-{index:08}.eventb"));
            if let Err(error) = std::fs::write(&file, &model.text) {
                eprintln!("error: {}: {error}", file.display());
                return ExitCode::FAILURE;
            }
        }
        match rossi::parse_components(&model.text) {
            Ok(_) => accepted += 1,
            Err(error) => {
                *rejections.entry(category_of(&error)).or_default() += 1;
                if args.show_rejected {
                    if !shown {
                        shown = true;
                        println!("--- first rejected model ---\n{}\n{error}\n---", model.text);
                    }
                    println!("REJECT\t{}", rejection_context(&model.text, &error));
                }
            }
        }
    }

    let percent = (accepted * 100).checked_div(args.count).unwrap_or(0);
    println!("accepted {accepted}/{} ({percent}%)", args.count);
    let mut ranked: Vec<(&str, usize)> = rejections.into_iter().collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(right.0)));
    for (category, count) in &ranked {
        println!("  {count:>6}  {category}");
    }
    ExitCode::SUCCESS
}

/// The name of a parse error's variant, used to bucket rejections.
///
/// Every rejection must land in a named bucket: an unclassified one means the
/// generator produced something nobody has looked at yet.
fn category_of(error: &rossi::ParseError) -> &'static str {
    use rossi::ParseError as E;
    match error {
        E::PestError { .. } => "PestError",
        E::UnexpectedRule { .. } => "UnexpectedRule",
        E::InvalidInteger(_) => "InvalidInteger",
        E::NestingTooDeep { .. } => "NestingTooDeep",
        E::ReservedWord { .. } => "ReservedWord",
        E::IncompatibleOperators { .. } => "IncompatibleOperators",
        E::AssignmentInPredicate { .. } => "AssignmentInPredicate",
        E::ExpressionNotBinding { .. } => "ExpressionNotBinding",
        E::InvalidTypeExpression { .. } => "InvalidTypeExpression",
        E::EmptyExpression => "EmptyExpression",
        E::EmptyPredicate => "EmptyPredicate",
        E::MissingPredicate => "MissingPredicate",
        E::MissingAction => "MissingAction",
        E::MissingVariable => "MissingVariable",
        E::MissingOperator => "MissingOperator",
        E::MissingValue => "MissingValue",
        E::AssignmentArityMismatch { .. } => "AssignmentArityMismatch",
        E::InvalidXml(_) => "InvalidXml",
        E::UnexpectedXmlRoot { .. } => "UnexpectedXmlRoot",
        E::MissingXmlAttribute { .. } => "MissingXmlAttribute",
        E::FileContext { source, .. } => category_of(source),
        E::UnsupportedIdentifier { .. } => "UnsupportedIdentifier",
        E::MalformedAttribute { .. } => "MalformedAttribute",
        E::IoError(_) => "IoError",
        E::ClauseError { .. } => "ClauseError",
        E::EmptyClause { .. } => "EmptyClause",
        E::ClauseOutOfOrder { .. } => "ClauseOutOfOrder",
        E::MissingFormula { .. } => "MissingFormula",
        E::MissingLabel { .. } => "MissingLabel",
        E::RecoverableError { .. } => "RecoverableError",
        E::ArityMismatch { .. } => "ArityMismatch",
        E::MultipleErrors(errors) => errors.first().map_or("MultipleErrors", category_of),
    }
}

/// The text around a parse error, for bucketing rejections by what they trip
/// over. Rossi reports a single failure point; the twenty-odd characters
/// before and after it are what identify the construct.
fn rejection_context(text: &str, error: &rossi::ParseError) -> String {
    let Some((line, column)) = error.position() else {
        return format!("[{}]", category_of(error));
    };
    let Some(source_line) = text.lines().nth(line.saturating_sub(1)) else {
        return "[no line]".to_string();
    };
    let chars: Vec<char> = source_line.chars().collect();
    let at = column.saturating_sub(1).min(chars.len());
    let start = at.saturating_sub(24);
    let end = (at + 12).min(chars.len());
    let before: String = chars[start..at].iter().collect();
    let after: String = chars[at..end].iter().collect();
    format!("{before} ⟪HERE⟫ {after}")
}
