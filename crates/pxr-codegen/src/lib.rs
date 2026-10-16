//! PXR code generation.
//!
//! * [`lower`] translates the typed AST produced by `pxr-sema` into the
//!   mid-level IR (`pxr-ir`). All implicit coercions, short-circuit evaluation
//!   and builtin expansions are made explicit here so that the IR is fully
//!   typed and the LLVM backend never has to guess.
//! * [`llvm`] translates the (optimized) IR into textual LLVM IR, which the
//!   driver hands to `clang` for machine-code generation and linking.

mod async_lower;
pub mod llvm;
pub mod lower;
mod roots;

pub use llvm::{emit_llvm, LlvmOptions};
pub use lower::lower_program;
