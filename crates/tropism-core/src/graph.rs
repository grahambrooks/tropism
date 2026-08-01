//! The internal module graph.
//!
//! Nodes are modules inside the project; edges are "imports". Kept separate from
//! the external package graph on purpose — see `design/02-data-model.md`.

use std::collections::{BTreeMap, VecDeque};

use petgraph::Direction;
use petgraph::graph::{DiGraph, NodeIndex};

/// How a module participates in the build.
///
/// Go forces this three-way split and it is not pedantry — collapsing any two of
/// them produces false cycles on real repositories:
///
/// * `Normal` — ordinary package code.
/// * `InternalTest` — `foo/x_test.go` declaring `package foo`. Compiled into
///   `foo`'s own test binary, so a path from here back to `foo` through other
///   packages *is* a cycle; Go rejects it as "import cycle not allowed in test".
/// * `ExternalTest` — `foo/x_test.go` declaring `package foo_test`. A genuinely
///   separate package that exists precisely so it can import `foo` and anything
///   else without creating a cycle. Never cycle-checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModuleKind {
    Normal,
    InternalTest,
    ExternalTest,
}

impl ModuleKind {
    pub fn is_test(self) -> bool {
        !matches!(self, Self::Normal)
    }
}

/// A module within a project, and how it participates in the build.
///
/// `project` is empty in a per-project graph, where it is implicit. The repo-wide
/// graph qualifies every node with it, so a cycle spanning packages can name the
/// modules involved rather than only the packages.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId {
    pub project: String,
    pub name: String,
    pub kind: ModuleKind,
}

impl ModuleId {
    pub fn module(name: impl Into<String>) -> Self {
        Self {
            project: String::new(),
            name: name.into(),
            kind: ModuleKind::Normal,
        }
    }

    pub fn internal_test(name: impl Into<String>) -> Self {
        Self {
            project: String::new(),
            name: name.into(),
            kind: ModuleKind::InternalTest,
        }
    }

    pub fn external_test(name: impl Into<String>) -> Self {
        Self {
            project: String::new(),
            name: name.into(),
            kind: ModuleKind::ExternalTest,
        }
    }

    /// Qualifies this module with the project that contains it.
    #[must_use]
    pub fn in_project(mut self, project: impl Into<String>) -> Self {
        self.project = project.into();
        self
    }

    /// The module whose test build this one belongs to.
    ///
    /// Only internal test modules have one. An external test package is separate
    /// from the package it tests, which is the entire reason Go offers it.
    pub fn under_test(&self) -> Option<ModuleId> {
        matches!(self.kind, ModuleKind::InternalTest).then(|| ModuleId::module(self.name.clone()))
    }
}

impl ModuleId {
    /// `project::module`, collapsing the cases where repeating both adds nothing:
    /// an unqualified node, a project's root module, and a module whose name is
    /// already the project name (routine in C#, where the namespace matches the
    /// assembly).
    fn qualified(&self) -> String {
        let project = if self.project.is_empty() {
            "."
        } else {
            self.project.as_str()
        };
        if self.project.is_empty() {
            self.name.clone()
        } else if self.name == "." || self.name == project {
            project.to_owned()
        } else {
            format!("{project}::{}", self.name)
        }
    }
}

impl std::fmt::Display for ModuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let base = self.qualified();
        match self.kind {
            ModuleKind::Normal => f.write_str(&base),
            ModuleKind::InternalTest => write!(f, "{base} [test]"),
            ModuleKind::ExternalTest => write!(f, "{base} [external test]"),
        }
    }
}

#[derive(Debug, Default)]
pub struct ModuleGraph {
    graph: DiGraph<ModuleId, ()>,
    index: BTreeMap<ModuleId, NodeIndex>,
}

impl ModuleGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_module(&mut self, id: ModuleId) -> NodeIndex {
        if let Some(existing) = self.index.get(&id) {
            return *existing;
        }
        let node = self.graph.add_node(id.clone());
        self.index.insert(id, node);
        node
    }

    /// Records that `from` imports `to`. Self-edges are dropped.
    pub fn add_edge(&mut self, from: ModuleId, to: ModuleId) {
        if from == to {
            return;
        }
        let (a, b) = (self.add_module(from), self.add_module(to));
        if !self.graph.contains_edge(a, b) {
            self.graph.add_edge(a, b, ());
        }
    }

    pub fn module_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Strongly connected components of size > 1 — one entry per tangle, not one
    /// per distinct cycle.
    ///
    /// Test-only nodes cannot appear: nothing imports them, so they have no
    /// incoming edges and can never be part of a component.
    pub fn cycles(&self) -> Vec<Vec<ModuleId>> {
        let mut components: Vec<Vec<ModuleId>> = petgraph::algo::tarjan_scc(&self.graph)
            .into_iter()
            .filter(|component| component.len() > 1)
            .map(|component| {
                let mut ids: Vec<ModuleId> = component
                    .into_iter()
                    .map(|node| self.graph[node].clone())
                    .collect();
                ids.sort();
                ids
            })
            .collect();

        components.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        components
    }

    /// Cycles that exist only in a package's own test build: a path from
    /// `pkg [test]` back to `pkg` through non-test modules.
    ///
    /// Go rejects these as "import cycle not allowed in test", so they are real
    /// defects — but they are not SCCs of the whole graph, because the test node
    /// has no incoming edges. Intermediate hops must be non-test modules: another
    /// package's test files are not compiled into this build.
    pub fn test_cycles(&self) -> Vec<Vec<ModuleId>> {
        let mut cycles: Vec<Vec<ModuleId>> = self
            .index
            .iter()
            .filter(|(id, _)| matches!(id.kind, ModuleKind::InternalTest))
            .filter_map(|(id, start)| {
                let target = self.index.get(&id.under_test()?)?;
                self.shortest_path(*start, *target)
            })
            .collect();

        cycles.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        cycles
    }

    /// Breadth-first path from `start` to `target`, refusing to route through
    /// test-only modules.
    fn shortest_path(&self, start: NodeIndex, target: NodeIndex) -> Option<Vec<ModuleId>> {
        let mut previous: BTreeMap<NodeIndex, NodeIndex> = BTreeMap::new();
        let mut queue = VecDeque::from([start]);
        let mut seen = vec![start];

        while let Some(node) = queue.pop_front() {
            for next in self.graph.neighbors_directed(node, Direction::Outgoing) {
                if seen.contains(&next) {
                    continue;
                }
                previous.insert(next, node);

                if next == target {
                    let mut path = vec![target];
                    let mut cursor = target;
                    while let Some(before) = previous.get(&cursor) {
                        path.push(*before);
                        cursor = *before;
                    }
                    path.reverse();
                    return Some(path.into_iter().map(|n| self.graph[n].clone()).collect());
                }

                // Only the starting node may be a test module.
                if !self.graph[next].kind.is_test() {
                    seen.push(next);
                    queue.push_back(next);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str) -> ModuleId {
        ModuleId::module(name)
    }

    fn build(edges: &[(ModuleId, ModuleId)]) -> ModuleGraph {
        let mut graph = ModuleGraph::new();
        for (from, to) in edges {
            graph.add_edge(from.clone(), to.clone());
        }
        graph
    }

    #[test]
    fn an_acyclic_graph_has_no_cycles() {
        let graph = build(&[(m("a"), m("b")), (m("b"), m("c"))]);
        assert!(graph.cycles().is_empty());
    }

    #[test]
    fn finds_a_two_module_cycle() {
        let graph = build(&[(m("a"), m("b")), (m("b"), m("a"))]);
        assert_eq!(graph.cycles(), vec![vec![m("a"), m("b")]]);
    }

    #[test]
    fn finds_a_three_module_cycle() {
        let graph = build(&[(m("a"), m("b")), (m("b"), m("c")), (m("c"), m("a"))]);
        assert_eq!(graph.cycles(), vec![vec![m("a"), m("b"), m("c")]]);
    }

    /// The reason cycles are reported per-SCC: many distinct cycles, one tangle.
    #[test]
    fn a_tangle_is_one_finding_not_many() {
        let graph = build(&[
            (m("a"), m("b")),
            (m("b"), m("c")),
            (m("c"), m("a")),
            (m("a"), m("c")),
            (m("c"), m("b")),
            (m("b"), m("a")),
        ]);
        assert_eq!(graph.cycles().len(), 1);
        assert_eq!(graph.cycles()[0].len(), 3);
    }

    #[test]
    fn separate_tangles_are_separate_findings_largest_first() {
        let graph = build(&[
            (m("x"), m("y")),
            (m("y"), m("x")),
            (m("a"), m("b")),
            (m("b"), m("c")),
            (m("c"), m("a")),
        ]);
        let cycles = graph.cycles();
        assert_eq!(cycles.len(), 2);
        assert_eq!(cycles[0].len(), 3, "largest tangle first");
    }

    #[test]
    fn self_edges_are_not_cycles() {
        let graph = build(&[(m("a"), m("a"))]);
        assert!(graph.cycles().is_empty());
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn duplicate_edges_are_recorded_once() {
        let graph = build(&[(m("a"), m("b")), (m("a"), m("b"))]);
        assert_eq!(graph.edge_count(), 1);
    }

    #[test]
    fn cycle_output_is_deterministic() {
        let forward = build(&[(m("a"), m("b")), (m("b"), m("c")), (m("c"), m("a"))]).cycles();
        let shuffled = build(&[(m("c"), m("a")), (m("b"), m("c")), (m("a"), m("b"))]).cycles();
        assert_eq!(forward, shuffled, "insertion order must not change output");
    }

    // --- test-build cycles ------------------------------------------------

    /// `scrape [test]` imports `teststorage`, which imports `scrape`. Go rejects
    /// this with "import cycle not allowed in test".
    #[test]
    fn finds_a_cycle_through_a_packages_own_test_files() {
        let mut graph = ModuleGraph::new();
        graph.add_module(m("scrape"));
        graph.add_edge(ModuleId::internal_test("scrape"), m("teststorage"));
        graph.add_edge(m("teststorage"), m("scrape"));

        let cycles = graph.test_cycles();
        assert_eq!(cycles.len(), 1);
        assert_eq!(
            cycles[0],
            vec![
                ModuleId::internal_test("scrape"),
                m("teststorage"),
                m("scrape")
            ]
        );
    }

    /// The Prometheus false positive. `scrape [test]` reaches `tsdb`, and only
    /// `tsdb`'s *test* files import `storage/remote` — which is not part of the
    /// build when tsdb is a dependency, so there is no cycle.
    #[test]
    fn a_path_through_another_packages_tests_is_not_a_cycle() {
        let mut graph = ModuleGraph::new();
        graph.add_module(m("scrape"));
        graph.add_edge(ModuleId::internal_test("scrape"), m("teststorage"));
        graph.add_edge(m("teststorage"), m("tsdb"));
        graph.add_edge(ModuleId::internal_test("tsdb"), m("storage/remote"));
        graph.add_edge(m("storage/remote"), m("scrape"));

        assert!(
            graph.test_cycles().is_empty(),
            "another package's test files are not in this build"
        );
        assert!(graph.cycles().is_empty());
    }

    #[test]
    fn a_test_module_that_imports_only_outward_is_fine() {
        let mut graph = ModuleGraph::new();
        graph.add_module(m("scrape"));
        graph.add_edge(ModuleId::internal_test("scrape"), m("helpers"));
        assert!(graph.test_cycles().is_empty());
    }

    /// Test-only nodes have no incoming edges, so they can never form an SCC.
    #[test]
    fn test_modules_never_appear_in_ordinary_cycles() {
        let mut graph = ModuleGraph::new();
        graph.add_edge(ModuleId::internal_test("a"), m("b"));
        graph.add_edge(m("b"), m("a"));
        assert!(graph.cycles().is_empty());
    }

    #[test]
    fn a_qualified_module_displays_project_and_module() {
        assert_eq!(
            ModuleId::module("api::handlers")
                .in_project("crates/web")
                .to_string(),
            "crates/web::api::handlers"
        );
    }

    /// Repeating the name adds nothing when the module *is* the project — the
    /// routine case in C#, where the namespace matches the assembly.
    #[test]
    fn a_qualified_module_collapses_redundant_names() {
        assert_eq!(
            ModuleId::module(".").in_project("Shop.Data").to_string(),
            "Shop.Data"
        );
        assert_eq!(
            ModuleId::module("Shop.Data")
                .in_project("Shop.Data")
                .to_string(),
            "Shop.Data"
        );
    }

    /// Two modules of the same name in different projects are different nodes.
    #[test]
    fn projects_namespace_their_modules() {
        let mut graph = ModuleGraph::new();
        graph.add_edge(
            ModuleId::module("util").in_project("a"),
            ModuleId::module("util").in_project("b"),
        );
        assert_eq!(graph.module_count(), 2);
        assert!(graph.cycles().is_empty());
    }

    #[test]
    fn module_id_displays_each_kind_distinctly() {
        assert_eq!(m("api").to_string(), "api");
        assert_eq!(ModuleId::internal_test("api").to_string(), "api [test]");
        assert_eq!(
            ModuleId::external_test("api").to_string(),
            "api [external test]"
        );
    }

    /// `package doc_test` importing `doc` is the designed use of an external test
    /// package. Cobra, Zerolog, and Prometheus all do it.
    #[test]
    fn an_external_test_package_importing_its_subject_is_not_a_cycle() {
        let mut graph = ModuleGraph::new();
        graph.add_module(m("doc"));
        graph.add_edge(ModuleId::external_test("doc"), m("doc"));
        assert!(graph.test_cycles().is_empty());
        assert!(graph.cycles().is_empty());
    }

    /// An external test package may reach its subject through other packages too;
    /// still not a cycle, because it is a separate package.
    #[test]
    fn an_external_test_package_reaching_its_subject_indirectly_is_not_a_cycle() {
        let mut graph = ModuleGraph::new();
        graph.add_module(m("promql"));
        graph.add_edge(ModuleId::external_test("promql"), m("promqltest"));
        graph.add_edge(m("promqltest"), m("promql"));
        assert!(graph.test_cycles().is_empty());
    }
}
