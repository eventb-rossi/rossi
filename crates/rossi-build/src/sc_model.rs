//! Public view of the typed model produced by the static checker.
//!
//! [`crate::build`] discards this model; [`crate::build_with_model`] returns
//! it so downstream passes (well-definedness, IDE tooling) can reach the
//! resolved type environments, axiom records and typed event chains without
//! re-deriving them from the emitted `.bcc`/`.bcm` XML.
//!
//! Only the record types are re-exported — the checker internals
//! (`check_context`, emission helpers) stay private to `crate::sc`.

pub use crate::sc::context_record::{
    AxiomDecl, CarrierSetDecl, ConstantDecl, ContextRecord, ExtendsDecl,
};
pub use crate::sc::machine_record::{
    ActionDecl, EventDecl, GuardDecl, InvariantDecl, MachineRecord, ParameterDecl,
    RefinesEventDecl, RefinesMachineDecl, SeesContextDecl, VariableDecl, VariantDecl, WitnessDecl,
};
pub use crate::sc::{CheckedContext, CheckedMachine, ScModel};
