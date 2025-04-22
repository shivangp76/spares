use std::collections::{HashMap, HashSet};

type NodeName = String;
type NodeRelations = Vec<(NodeName, NodeName)>;

#[derive(Debug, Default)]
pub struct Node {
    children: HashSet<NodeName>,
}

pub fn build_tree(relations: &NodeRelations) -> HashMap<NodeName, Node> {
    let mut tree: HashMap<NodeName, Node> = HashMap::new();
    let mut children = HashSet::new();

    for (parent, child) in relations {
        tree.entry(parent.clone())
            .or_default()
            .children
            .insert(child.clone());
        children.insert(child.clone());
    }

    // Find roots (tags that are never children)
    let roots: HashSet<_> = tree.keys().cloned().collect();
    let roots: HashSet<_> = roots.difference(&children).cloned().collect();

    // Ensure all tags are in the tree (including roots)
    for root in &roots {
        tree.entry(root.clone()).or_default();
    }

    tree
}

pub fn print_tree(tree: &HashMap<NodeName, Node>, tag: &NodeName, indent: usize) {
    println!("{:indent$}{}", "", tag, indent = indent * 2);
    if let Some(node) = tree.get(tag) {
        for child in &node.children {
            print_tree(tree, child, indent + 1);
        }
    }
}

// Usage:
// let tag_relations_str: Vec<(&str, &str)> = vec![
//     // (parent_tag, child_tag)
//     ("parent-1", "child-1"),
//     ("", "parent-1"),
// ];
// let tag_relations = tag_relations_str
//     .into_iter()
//     .map(|(a, b)| (a.to_string(), b.to_string()))
//     .collect::<Vec<_>>();
// let tree = build_tree(&tag_relations);
// for root in tree
//     .keys()
//     .filter(|&tag| tag_relations.iter().all(|(_, child)| child != tag))
// {
//     print_tree(&tree, root, 0);
// }
