use crate::dependency_provider::UvDependencyProvider;
use crate::pubgrub::PubGrubPackage;
use crate::resolution::{AnnotatedDist, ResolutionGraphNode};
use crate::ResolutionGraph;
use petgraph::{Direction, Graph};
use pubgrub::{Kind, Ranges, SelectedDependencies, State};
use rustc_hash::FxHashSet;
use std::collections::VecDeque;
use uv_distribution_types::{Dist, DistRef, ResolvedDist, SourceDist};
use uv_normalize::PackageName;
use uv_pep440::Version;

/// A chain of derivation steps from the root package to the current package, to explain why a
/// package is included in the resolution.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct DerivationChain(Vec<DerivationStep>);

impl FromIterator<DerivationStep> for DerivationChain {
    fn from_iter<T: IntoIterator<Item = DerivationStep>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl DerivationChain {
    /// Compute a [`DerivationChain`] from a resolution graph.
    pub fn from_graph(graph: &ResolutionGraph, target: DistRef<'_>) -> Option<Self> {
        // Figure out why a distribution was included in the resolution.
        let target = graph
            .petgraph
            .node_indices()
            .find(|node| {
                let ResolutionGraphNode::Dist(AnnotatedDist {
                    dist: ResolvedDist::Installable(dist),
                    ..
                }) = &graph.petgraph[*node]
                else {
                    return false;
                };
                target == dist.as_ref()
            })
            .expect("every distribution in the resolution graph should be present");

        // Perform a BFS to find the shortest path to the root.
        let mut queue = VecDeque::new();
        queue.push_back((target, Vec::new()));

        let mut seen = FxHashSet::default();
        while let Some((node, mut path)) = queue.pop_front() {
            if !seen.insert(node) {
                continue;
            }
            match &graph.petgraph[node] {
                ResolutionGraphNode::Root => {
                    path.reverse();
                    path.pop();
                    return Some(Self::from_iter(path));
                }
                ResolutionGraphNode::Dist(AnnotatedDist { name, version, .. }) => {
                    path.push(DerivationStep::new(name.clone(), version.clone()));
                    for neighbor in graph.petgraph.neighbors_directed(node, Direction::Incoming) {
                        queue.push_back((neighbor, path.clone()));
                    }
                }
            }
        }

        None
    }

    /// Compute a [`DerivationChain`] from the current PubGrub state.
    pub fn from_state(
        package: &PubGrubPackage,
        version: &Version,
        state: &State<UvDependencyProvider>,
    ) -> Option<Self> {
        /// Find a path from the current package to the root package.
        fn fill_complete_path<'state, 'data>(
            package: &'data PubGrubPackage,
            version: &'data Version,
            state: &'state State<UvDependencyProvider>,
            solution: &'state SelectedDependencies<UvDependencyProvider>,
            path: &mut Vec<(
                &'data PubGrubPackage,
                &'data Ranges<Version>,
                &'data PubGrubPackage,
                &'data Ranges<Version>,
                &'data Version,
            )>,
        ) -> bool
        where
            'state: 'data,
        {
            // If we've reached the "Root" package, return the path as a solution.
            if package.is_root() {
                return true;
            }

            // Get the incompatibilities for the current package.
            if let Some(incompats) = state.incompatibilities.get(package) {
                for i in incompats {
                    let incompat = &state.incompatibility_store[*i];

                    // Check if this incompatibility has a valid dependency chain.
                    if let Kind::FromDependencyOf(p1, v1, p2, v2) = &incompat.kind {
                        if p2 == package && v2.contains(&version) {
                            // Try to get the next package and version.
                            if let Some(version) = solution.get(p1) {
                                // Add to the current path.
                                path.push((p1, v1, p2, v2, version));

                                // Recursively search the next package.
                                if fill_complete_path(p1, version, state, solution, path) {
                                    return true;
                                }

                                // Backtrack if the path didn't lead to "Root."
                                path.pop();
                            }
                        }
                    }
                }
            }
            false
        }

        let solution = state.partial_solution.extract_solution();
        let path = {
            let mut path = vec![];
            if !fill_complete_path(package, version, &state, &solution, &mut path) {
                return None;
            }
            path
        };

        Some(
            path.into_iter()
                .rev()
                .filter_map(|(p1, v1, p2, v2, version)| {
                    let name = p1.name()?;
                    Some(DerivationStep::new(name.clone(), version.clone()))
                })
                .collect(),
        )
    }

    /// Returns the length of the derivation chain.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if the derivation chain is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns an iterator over the steps in the derivation chain.
    pub fn iter(&self) -> std::slice::Iter<DerivationStep> {
        self.0.iter()
    }
}

impl std::fmt::Display for DerivationChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (idx, step) in self.0.iter().enumerate() {
            if idx > 0 {
                write!(f, " -> ")?;
            }
            write!(f, "{}=={}", step.name, step.version)?;
        }
        Ok(())
    }
}

impl IntoIterator for DerivationChain {
    type Item = DerivationStep;
    type IntoIter = std::vec::IntoIter<DerivationStep>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// A step in a derivation chain.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DerivationStep {
    /// The name of the package.
    name: PackageName,
    /// The version of the package.
    version: Version,
}

impl DerivationStep {
    /// Create a [`DerivationStep`] from a package name and version.
    pub fn new(name: PackageName, version: Version) -> Self {
        Self { name, version }
    }
}

impl std::fmt::Display for DerivationStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}=={}", self.name, self.version)
    }
}
