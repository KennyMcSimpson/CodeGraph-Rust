//! Synthesised edges for dynamic dispatch (design doc 7).

use crate::driver::ResolveOutput;
use crate::engine::Engine;
use cg_store::EdgeBuild;

#[derive(Default)]
pub struct SynthOutput {
    pub edges: Vec<EdgeBuild>,
    pub emeta: Vec<String>,
}

pub fn run_all(_e: &Engine<'_>, _resolved: &ResolveOutput) -> SynthOutput {
    SynthOutput::default()
}
