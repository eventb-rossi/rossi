//! Rodin's well-definedness calculus.
//!
//! A faithful port of the machinery eventb-checker borrows from Rodin to
//! decide whether a formula is well-defined: the L-operator that derives a
//! formula's WD lemma ([`computer`]), the `FormulaBuilder` smart
//! constructors that keep the lemma in Rodin's normal form ([`builder`]),
//! the `WDImprover` simplifier that drops trivial and subsumed conjuncts
//! ([`mod@improve`]), binder flattening and capture-aware renaming
//! ([`normal`]), and the `Formula#toString` renderer that prints a lemma
//! byte-identically to Rodin ([`render`]).
//!
//! This module is the calculus only; turning its lemmas into diagnostics
//! over a checked project is layered on top.

pub mod builder;
pub mod computer;
pub mod improve;
pub mod normal;
pub mod render;
