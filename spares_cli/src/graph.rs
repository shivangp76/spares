use std::collections::HashMap;

use petgraph::Graph;
use petgraph::dot::Config;
use petgraph::dot::Dot;
use petgraph::graph::NodeIndex;
use spares_core::model::LinkRelation;
use spares_core::model::NoteId;
use spares_core::schema::note::LinkedNote;
use spares_core::schema::note::NoteResponse;

/// Usage: Paste output into <https://dreampuf.github.io/GraphvizOnline/> with engine set to `osage`.
pub(crate) fn chart(note_responses: Vec<NoteResponse>) {
    println!("{}", dot(note_responses));
}

/// The notes and the links between them, in Graphviz's DOT language. A note linked to but not
/// among `note_responses` gets a node of its own.
fn dot(note_responses: Vec<NoteResponse>) -> String {
    let mut nodes = HashMap::<NoteId, NodeIndex>::new();
    let mut graph = Graph::<String, String>::new();

    // Add all nodes
    for note_response in &note_responses {
        let node = graph.add_node(note_response.id.letters());
        nodes.insert(note_response.id, node);
    }

    // Add edges
    let mut edges: Vec<(NodeIndex, NodeIndex)> = Vec::new();
    for note_response in note_responses {
        if let Some(linked_notes) = note_response.linked_notes {
            let linked_notes_filtered = linked_notes
                .into_iter()
                .filter(|x| x.linked_note_id.is_some())
                .collect::<Vec<_>>();
            for LinkedNote {
                linked_note_id,
                relation,
                ..
            } in linked_notes_filtered
            {
                let linked_note_id = linked_note_id.unwrap();
                let to_node = *nodes
                    .entry(linked_note_id)
                    .or_insert_with(|| graph.add_node(linked_note_id.letters()));
                let from_node = nodes[&note_response.id];
                // Each edge points from a note to a note it builds on
                match relation {
                    LinkRelation::Prerequisite => edges.push((from_node, to_node)),
                    LinkRelation::Dependent => edges.push((to_node, from_node)),
                    LinkRelation::Related => {}
                }
            }
        }
    }
    graph.extend_with_edges(&edges);

    format!("{:?}", Dot::with_config(&graph, &[Config::EdgeNoLabel]))
}

#[cfg(test)]
mod tests {
    use spares_core::model::LinkRelation;
    use spares_core::model::NoteId;
    use spares_core::schema::note::LinkedNote;
    use spares_core::schema::note::NoteResponse;

    fn note(id: &str, links: &[(&str, LinkRelation)]) -> NoteResponse {
        serde_json::from_value(serde_json::json!({
            "id": id.parse::<NoteId>().unwrap(),
            "data": "",
            "parser_id": 1,
            "parser_name": "typst",
            "keywords": [],
            "tags": [],
            "custom_data": {},
            "linked_notes": links
                .iter()
                .map(|(to, relation)| LinkedNote {
                    searched_keyword: (*to).to_string(),
                    linked_note_id: Some(to.parse().unwrap()),
                    matched_keyword: Some((*to).to_string()),
                    relation: *relation,
                })
                .collect::<Vec<_>>(),
            "card_count": 0,
            "created_at": "2026-11-26T00:00:00Z",
            "updated_at": "2026-11-26T00:00:00Z",
        }))
        .unwrap()
    }

    /// A linked note that isn't exported gets its own node, instead of the lookup of its node
    /// panicking, and edges point from a note to a note it builds on.
    #[test]
    fn links_to_notes_outside_the_export_get_their_own_nodes() {
        let dot = super::dot(vec![
            note("a", &[("b", LinkRelation::Prerequisite)]),
            note(
                "c",
                &[("d", LinkRelation::Dependent), ("a", LinkRelation::Related)],
            ),
        ]);
        for letters in ["a", "c", "b", "d"] {
            assert!(
                dot.contains(&format!(r#"label = "\"{letters}\"""#)),
                "{dot}"
            );
        }
        // Nodes in order a, c, b, d: a -> b, and d -> c for the dependent link
        assert!(dot.contains("0 -> 2"), "{dot}");
        assert!(dot.contains("3 -> 1"), "{dot}");
        assert_eq!(dot.matches("->").count(), 2, "{dot}");
    }
}
