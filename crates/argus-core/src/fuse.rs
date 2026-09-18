//! Fusion of normalized evidence from several sources.

use argus_fusion::{ElementEvidence, Fusion};

use crate::normalize::Normalized;

/// Elements fused from normalized evidence.
///
/// Only [`fuse`] can create this type, and observations are assembled only
/// from it, so every element of an observation went through normalization
/// and fusion.
#[derive(Debug, Clone, PartialEq)]
pub struct Fused(pub(crate) Fusion);

impl Fused {
    /// Evidence for each element, in the order of the assembled
    /// observation's elements.
    pub fn evidence(&self) -> impl Iterator<Item = &ElementEvidence> {
        self.0.elements.iter().map(|element| &element.evidence)
    }

    /// Takes the evidence for each element, in the order of the assembled
    /// observation's elements.
    pub fn into_evidence(self) -> Vec<ElementEvidence> {
        self.0.elements.into_iter().map(|element| element.evidence).collect()
    }
}

/// Fuses the normalized output of one or more sources (see
/// [`argus_fusion`] for the policy). A single source passes through
/// unchanged.
pub fn fuse(sources: &[Normalized]) -> Fused {
    Fused(argus_fusion::fuse(sources.iter().flat_map(Normalized::candidates)))
}
