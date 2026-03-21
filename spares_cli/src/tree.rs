use std::{collections::HashMap, fmt::Write};

#[derive(Default)]
pub(crate) struct TreeNode {
    children: HashMap<String, TreeNode>,
}

impl TreeNode {
    fn insert(&mut self, path: Vec<String>) {
        if path.is_empty() {
            return;
        }

        let mut current = self;
        for part in path {
            current = current.children.entry(part).or_default();
        }
    }

    // fn print(&self, indent: usize) {
    //     let mut keys: Vec<_> = self.children.keys().collect();
    //     keys.sort();
    //
    //     for key in keys {
    //         println!("{}{}", " ".repeat(indent), key);
    //         self.children[key].print(indent + 2);
    //     }
    // }
}

pub(crate) fn build_tree(strings: Vec<String>) -> TreeNode {
    let mut root = TreeNode::default();

    for s in strings {
        let parts: Vec<String> = s.split(':').map(|p| p.to_string()).collect();
        root.insert(parts);
    }

    root
}

pub(crate) fn tree_to_string(node: &TreeNode, indent: usize) -> String {
    let mut result = String::new();
    let mut keys: Vec<_> = node.children.keys().collect();
    keys.sort();

    for key in keys {
        writeln!(&mut result, "{}{}", " ".repeat(indent), key).unwrap();
        result.push_str(&tree_to_string(&node.children[key], indent + 2));
    }

    result
}

// fn main() {
//     // Basic example with common parents
//     let tree = vec![
//         "root:child1:grandchild1".to_string(),
//         "root:child1:grandchild2".to_string(),
//         "root:child2".to_string(),
//         "root:child2:grandchild3".to_string(),
//         "another_root:child:grandchild".to_string(),
//     ];
//
//     println!("Collapsed tree:");
//     build_tree(tree).print(0);
// }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_complex_tree_with_shared_parents() {
        // Test comprehensive tree structure with:
        // - Multiple roots
        // - Shared parent paths
        // - Different depth levels
        // - Overlapping paths where some are prefixes of others
        let input = vec![
            "company:engineering:backend:auth".to_string(),
            "company:engineering:backend:api".to_string(),
            "company:engineering:frontend:ui".to_string(),
            "company:engineering:frontend:components".to_string(),
            "company:hr:recruiting".to_string(),
            "company:hr:benefits".to_string(),
            "startup:engineering:fullstack".to_string(),
            "company:engineering".to_string(), // Shorter overlapping path
        ];

        let tree = build_tree(input);
        let output = tree_to_string(&tree, 0);

        let expected = "company
  engineering
    backend
      api
      auth
    frontend
      components
      ui
  hr
    benefits
    recruiting
startup
  engineering
    fullstack
";
        assert_eq!(output, expected);
    }

    #[test]
    fn test_edge_cases_and_special_scenarios() {
        // Test edge cases including:
        // - Empty strings and empty parts (consecutive colons)
        // - Duplicate paths (should only appear once)
        // - Single-level nodes
        // - Trailing/leading colons
        // - Mixed depth levels from same root
        let input = vec![
            "root:a:b".to_string(),
            "root:a:b".to_string(), // Duplicate
            "root:a:c".to_string(),
            "root::empty".to_string(), // Empty middle part
            "single".to_string(),      // No colons
            "root:".to_string(),       // Trailing colon
            ":leading".to_string(),    // Leading colon
            "root:a".to_string(),      // Overlapping shorter path
            "x:y:z:deep".to_string(),
            "x:y".to_string(),
        ];

        let tree = build_tree(input);
        let output = tree_to_string(&tree, 0);

        let expected = "\n  leading\nroot\n  \n    empty\n  a\n    b\n    c\nsingle\nx\n  y\n    z\n      deep\n";
        assert_eq!(output, expected);
    }
}
