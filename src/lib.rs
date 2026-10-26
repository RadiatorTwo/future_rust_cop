//! Future Cop: L.A.P.D. (1998) recompiled to native code with Cranelift.
//!
//! The player's FCopLAPD.exe is decoded (iced-x86), every function is lifted to Cranelift IR
//! and compiled to host code; game pointers stay 32-bit in a 4 GB guest address space.

pub mod discover;
pub mod image;
pub mod jit;
pub mod lift;
pub mod mpeg;
pub mod platform;
pub mod rt;
pub mod smc;
