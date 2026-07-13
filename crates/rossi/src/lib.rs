//! # Rossi
//!
//! A modern parser for the Event-B formal modeling language.
//!
//! This library provides a parser that can read Event-B models (Contexts and Machines)
//! and convert them into a structured Abstract Syntax Tree (AST) for further processing.
//!
//! ## Example
//!
//! ```no_run
//! use rossi::parse;
//!
//! let source = r#"
//! CONTEXT counter_ctx
//! SETS
//!     STATUS
//! CONSTANTS
//!     max_value
//! AXIOMS
//!     @axm1 max_value = 100
//! END
//! "#;
//!
//! let component = parse(source).unwrap();
//! ```
//!
//! ## Features
//!
//! - Full Event-B syntax support (Contexts and Machines)
//! - Rich AST representation
//! - Pretty printer (AST back to text)
//! - Detailed error messages
//! - Optional serde support for serialization
//!
//! ## Architecture
//!
//! The parser is built using the [pest](https://pest.rs/) parser generator, which provides:
//! - Fast parsing with PEG (Parsing Expression Grammar)
//! - Clear error messages
//! - Maintainable grammar definition
//!

pub mod ast;
pub mod builtins;
pub(crate) mod comment_attach;
pub mod comments;
pub mod deps;
pub mod error;
pub mod keywords;
pub mod names;
pub mod nesting;
pub mod op_info;
pub mod operators;
pub mod parser;
pub mod pretty;
pub mod selection;
pub mod snippets;
pub mod xml;

// Re-export main types for convenience
pub use ast::{
    Action, ActionKind, AtomicBuiltinKind, BuiltinFunction, BuiltinPredicate, Component, Context,
    Event, EventStatus, Expression, ExpressionKind, FileMetadata, Ident, IdentPattern,
    InitialisationEvent, LabeledAction, LabeledPredicate, Machine, NamedElement, Predicate,
    PredicateKind, SetDeclaration, TypedIdentifier,
};
pub use deps::{ComponentKind, DependencyGraph, EdgeKind};
pub use error::{ParseError, ParseResult, Result};
pub use nesting::MAX_NESTING_DEPTH;
pub use parser::{
    component_name_occurrences, parse, parse_action_str, parse_components,
    parse_components_with_recovery, parse_expression_str, parse_predicate_str, parse_with_recovery,
};
pub use pretty::{
    PrettyPrinter, components_to_string, components_to_string_ascii, format_str, to_string,
    to_string_ascii,
};
pub use selection::enclosing_spans;
pub use xml::{
    NamedComponent, NamedProject, component_filename, parse_xml, parse_zip, parse_zip_file,
    parse_zip_file_with_recovery, parse_zip_with_recovery, read_project_name, to_multi_project_zip,
    to_project_zip, to_xml, to_zip, write_multi_project_directory, write_project_directory,
    write_project_zip_file, write_zip_file,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_context() {
        let source = r#"
        CONTEXT simple
        END
        "#;

        let result = parse(source);
        assert!(result.is_ok());

        if let Component::Context(ctx) = result.unwrap() {
            assert_eq!(ctx.name, "simple");
        } else {
            panic!("Expected Context component");
        }
    }

    #[test]
    fn test_parse_context_with_sets() {
        let source = r#"
        CONTEXT ctx
        SETS
            PERSON STATUS
        END
        "#;

        let result = parse(source);
        assert!(result.is_ok(), "Parse error: {:?}", result.as_ref().err());

        if let Component::Context(ctx) = result.unwrap() {
            assert_eq!(ctx.name, "ctx");
            assert_eq!(ctx.sets.len(), 2);
            assert_eq!(ctx.sets[0].name(), "PERSON");
            assert_eq!(ctx.sets[1].name(), "STATUS");
        } else {
            panic!("Expected Context component");
        }
    }

    #[test]
    fn test_parse_simple_machine() {
        let source = r#"
        MACHINE counter
        END
        "#;

        let result = parse(source);
        assert!(result.is_ok());

        if let Component::Machine(m) = result.unwrap() {
            assert_eq!(m.name, "counter");
        } else {
            panic!("Expected Machine component");
        }
    }

    #[test]
    fn test_parse_machine_with_variables() {
        let source = r#"
        MACHINE counter
        VARIABLES
            count
        END
        "#;

        let result = parse(source);
        assert!(result.is_ok());

        if let Component::Machine(m) = result.unwrap() {
            assert_eq!(m.name, "counter");
            assert_eq!(m.variables.len(), 1);
            assert_eq!(m.variables[0].name, "count");
        } else {
            panic!("Expected Machine component");
        }
    }
}
