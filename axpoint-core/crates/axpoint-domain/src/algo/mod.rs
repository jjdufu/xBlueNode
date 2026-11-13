//! Pure `petgraph`-backed graph algorithms (`PageRank`, degree centrality,
//! deterministic label propagation, connected components, and weighted shortest paths).

use petgraph::graph::DiGraph;
use uuid::Uuid;

/// Canonical directed weighted business graph (`node = Uuid`, `edge = weight`).
pub type WeightedDiGraph = DiGraph<Uuid, f64>;
