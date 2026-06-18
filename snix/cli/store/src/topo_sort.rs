use std::collections::HashMap;

use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};

/// Reorder `items` so every node comes after the nodes it references (topological order).
/// `digest`/`refs` read a node's own key and its reference keys. Reference cycles (which a store
/// DAG never has, barring exotic self-references) fall back to input order.
pub fn topologically_sorted<T>(
    items: Vec<T>,
    digest: impl Fn(&T) -> [u8; 20],
    refs: impl Fn(&T) -> Vec<[u8; 20]>,
) -> Vec<T> {
    // Node weight is the item's index. Edges point reference -> referrer, so toposort yields a
    // path after everything it references.
    let mut graph = DiGraph::<usize, ()>::new();
    let node_of: HashMap<[u8; 20], NodeIndex> = items
        .iter()
        .enumerate()
        .map(|(i, x)| (digest(x), graph.add_node(i)))
        .collect();
    for x in &items {
        let referrer = node_of[&digest(x)];
        for r in refs(x) {
            if let Some(&dep) = node_of.get(&r) {
                if dep != referrer {
                    graph.add_edge(dep, referrer, ());
                }
            }
        }
    }

    let order: Vec<usize> = match toposort(&graph, None) {
        Ok(sorted) => sorted.iter().map(|&n| graph[n]).collect(),
        Err(_) => (0..items.len()).collect(),
    };

    let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}

#[cfg(test)]
mod tests {
    use super::topologically_sorted;

    fn key(s: &str) -> [u8; 20] {
        let mut k = [0u8; 20];
        k[..s.len()].copy_from_slice(s.as_bytes());
        k
    }

    #[test]
    fn orders_references_before_referrers() {
        // Input in referrer-first order: a references b, b references c.
        let items = vec![("a", vec!["b"]), ("b", vec!["c"]), ("c", vec![])];
        let out = topologically_sorted(
            items,
            |item| key(item.0),
            |item| item.1.iter().map(|r| key(r)).collect(),
        );
        let names: Vec<&str> = out.iter().map(|item| item.0).collect();
        assert_eq!(names, vec!["c", "b", "a"]);
    }
}
