use serde::{Deserialize, Serialize};

/// Where the evidence for an element came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Source {
    /// The platform accessibility tree.
    Accessibility,
    /// Optical character recognition over pixels.
    Ocr,
    /// Visual UI detection over pixels.
    Vision,
    /// Structure exported by the application itself.
    ApplicationApi,
    /// Inferred by Argus from other evidence (e.g. layout analysis).
    Derived,
    /// A source this version of the protocol does not know.
    ///
    /// Only produced when parsing data from a newer producer; Argus never
    /// emits it.
    #[serde(other)]
    Unknown,
}
