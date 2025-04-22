use petgraph::Graph;
use petgraph::dot::{Config, Dot};
use petgraph::graph::NodeIndex;
use spares::schema::note::{LinkedNote, NoteResponse};
use std::collections::HashMap;

/// Usage: Paste output into <https://dreampuf.github.io/GraphvizOnline/> with engine set to `osage`.
pub fn chart(note_responses: Vec<NoteResponse>) {
    let mut nodes = HashMap::<i64, NodeIndex>::new();
    let mut graph = Graph::<String, String>::new();

    // Add all nodes
    for note_response in &note_responses {
        let node = graph.add_node(format!("{}", note_response.id));
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
            for LinkedNote { linked_note_id, .. } in linked_notes_filtered {
                if !nodes.contains_key(&linked_note_id.unwrap()) {
                    let node = graph.add_node(format!("{}", linked_note_id.unwrap()));
                    nodes.insert(note_response.id, node);
                }
                let from_node = nodes.get(&note_response.id).unwrap();
                let to_node = nodes.get(&linked_note_id.unwrap()).unwrap();
                edges.push((*from_node, *to_node));
            }
        }
    }
    graph.extend_with_edges(&edges);

    println!("{:?}", Dot::with_config(&graph, &[Config::EdgeNoLabel]));
}
