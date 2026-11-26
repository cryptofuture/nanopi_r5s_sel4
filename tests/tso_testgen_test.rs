//! Host-test shim: compiles the shared TSO module and the board-side TSO
//! test source at one crate root so `crate::tso` resolves exactly as it
//! does inside the driver PDs.
#[path = "../pds/gmac_dma_corrected/src/tso.rs"]
mod tso;
#[path = "../pds/gmac_dma_corrected/src/tso_testgen.rs"]
mod tso_testgen;
