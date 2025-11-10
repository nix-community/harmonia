#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RLookup {
    Node(usize),
    Edge(usize, usize),
}

impl Default for RLookup {
    fn default() -> Self {
        Self::Node(0)
    }
}

const fn min(v1: usize, v2: usize) -> usize {
    if v1 <= v2 { v1 } else { v2 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RMatch {
    Matched(usize),
    NeedsMore,
    Mismatch(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct REdge {
    prefix: &'static [u8],
    node_idx: usize,
}

impl REdge {
    const fn new() -> Self {
        REdge {
            prefix: b"",
            node_idx: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RNode<R> {
    Branch(usize, usize),
    Leaf(R),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RTree<const N: usize, const E: usize, const B: usize, R> {
    nodes: [RNode<R>; N],
    nodes_len: usize,
    edges: [REdge; E],
    edges_len: usize,
}

impl<const N: usize, const E: usize, const B: usize, R> RTree<N, E, B, R>
where
    R: Copy,
{
    pub(crate) const fn new() -> Self {
        RTree {
            nodes: [RNode::Branch(0, 0); N],
            nodes_len: 0,
            edges: [REdge::new(); E],
            edges_len: 0,
        }
    }

    const fn add_leaf_node(&mut self, value: R) -> usize {
        let idx = self.nodes_len;
        let node = &mut self.nodes[idx];
        *node = RNode::Leaf(value);
        self.nodes_len += 1;
        idx
    }

    const fn add_branch_node(&mut self, edge_idx: usize, len: usize) -> usize {
        let idx = self.nodes_len;
        let node = &mut self.nodes[idx];
        *node = RNode::Branch(edge_idx, len);
        self.nodes_len += 1;
        idx
    }

    const fn add_edge_list(&mut self, node_idx: usize, prefix: &'static [u8]) -> (usize, usize) {
        let idx = self.edges_len;
        let edge = &mut self.edges[idx];
        edge.node_idx = node_idx;
        edge.prefix = prefix;
        self.edges_len += B;
        (idx, 1)
    }

    const fn add_edge(&mut self, node_idx: usize, edge_node_idx: usize, prefix: &'static [u8]) {
        let node = &mut self.nodes[node_idx];
        match node {
            RNode::Branch(edge_idx, len) => {
                if *len == B {
                    panic!("Branch factor exceeded");
                }
                let edge = &mut self.edges[*edge_idx + *len];
                edge.node_idx = edge_node_idx;
                edge.prefix = prefix;
                *len += 1;
            }
            RNode::Leaf(_) => panic!("Adding edge to leaf"),
        }
    }

    const fn update_edge(&mut self, edge_idx: usize, node_idx: usize, prefix: &'static [u8]) {
        let edge = &mut self.edges[edge_idx];
        edge.node_idx = node_idx;
        edge.prefix = prefix;
    }

    pub const fn insert(&mut self, token: &'static [u8], value: R) {
        if self.nodes_len == 0 {
            let (edge_idx, len) = self.add_edge_list(1, token);
            self.add_branch_node(edge_idx, len);
            self.add_leaf_node(value);
            return;
        }
        let mut state = RLookup::Node(0);
        match self.lookup(&mut state, token) {
            RMatch::Matched(_) => {
                panic!("shared prefix");
            }
            RMatch::NeedsMore => {
                panic!("shared prefix");
            }
            RMatch::Mismatch(idx) => {
                let (_, postfix) = token.split_at(idx);
                match state {
                    RLookup::Node(node_idx) => {
                        let new_node_idx = self.add_leaf_node(value);
                        self.add_edge(node_idx, new_node_idx, postfix);
                    }
                    RLookup::Edge(edge_idx, read) => {
                        let edge = &self.edges[edge_idx];
                        let node_idx = edge.node_idx;
                        let (shared, left_prefix) = edge.prefix.split_at(read);

                        let (left_edge_idx, len) = self.add_edge_list(node_idx, left_prefix);
                        let branch_node_idx = self.add_branch_node(left_edge_idx, len);

                        let leaf_node_idx = self.add_leaf_node(value);
                        self.add_edge(branch_node_idx, leaf_node_idx, postfix);

                        self.update_edge(edge_idx, branch_node_idx, shared);
                    }
                }
            }
        }
    }

    pub const fn get_value(&self, state: &RLookup) -> R {
        match state {
            RLookup::Node(node_idx) => {
                let node = &self.nodes[*node_idx];
                match node {
                    RNode::Leaf(value) => *value,
                    RNode::Branch(_, _) => panic!("invalid state to get value. must be leaf node"),
                }
            }
            RLookup::Edge(_, _) => panic!("invalid state to get value. must be node"),
        }
    }

    pub const fn lookup(&self, state: &mut RLookup, mut buf: &[u8]) -> RMatch {
        let mut parsed = 0;
        'state: loop {
            match *state {
                RLookup::Node(node_idx) => {
                    let node = &self.nodes[node_idx];
                    match node {
                        RNode::Leaf(_) => {
                            return RMatch::Matched(parsed);
                        }
                        RNode::Branch(edge_idx, len) => {
                            if buf.is_empty() {
                                return RMatch::NeedsMore;
                            }
                            let mut start = *edge_idx;
                            let end = start + *len;
                            while start < end {
                                let edge = &self.edges[start];
                                if buf[0] == edge.prefix[0] {
                                    let (_, rest) = buf.split_at(1);
                                    buf = rest;
                                    parsed += 1;
                                    *state = RLookup::Edge(start, 1);
                                    continue 'state;
                                }
                                start += 1;
                            }
                            return RMatch::Mismatch(parsed);
                        }
                    }
                }
                RLookup::Edge(edge_idx, mut read) => {
                    let edge = &self.edges[edge_idx];
                    let cmp = min(edge.prefix.len() - read, buf.len());
                    let mut idx = 0;
                    while idx < cmp {
                        if buf[idx] != edge.prefix[read] {
                            parsed += idx;
                            *state = RLookup::Edge(edge_idx, read);
                            return RMatch::Mismatch(parsed);
                        }
                        idx += 1;
                        read += 1;
                    }
                    if read < edge.prefix.len() {
                        *state = RLookup::Edge(edge_idx, read);
                        return RMatch::NeedsMore;
                    } else {
                        let (_, rest) = buf.split_at(cmp);
                        buf = rest;
                        parsed += cmp;
                        *state = RLookup::Node(edge.node_idx);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod unittests {
    use rstest::rstest;

    use crate::archive::read_nar::{TOK_DIR, TOK_FILE, TOK_FILE_E, TOK_SYM};

    use super::{REdge, RLookup, RMatch, RNode, RTree};

    const NODE_SELECT: RTree<7, 6, 2, Node> = {
        let (sym_pre, sym_post) = TOK_SYM.split_at(8);
        let (_, file) = TOK_FILE.split_at(8);
        let (file_pre, file_post) = file.split_at(8);
        let (_, exec_post) = TOK_FILE_E.split_at(16);
        RTree {
            nodes: [
                RNode::Branch(0, 2),
                RNode::Leaf(Node::Directory),
                RNode::Leaf(Node::Symlink),
                RNode::Branch(2, 2),
                RNode::Leaf(Node::File),
                RNode::Branch(4, 2),
                RNode::Leaf(Node::ExecutableFile),
            ],
            nodes_len: 7,
            edges: [
                REdge {
                    prefix: TOK_DIR,
                    node_idx: 1,
                },
                REdge {
                    prefix: sym_pre,
                    node_idx: 3,
                },
                REdge {
                    prefix: sym_post,
                    node_idx: 2,
                },
                REdge {
                    prefix: file_pre,
                    node_idx: 5,
                },
                REdge {
                    prefix: file_post,
                    node_idx: 4,
                },
                REdge {
                    prefix: exec_post,
                    node_idx: 6,
                },
            ],
            edges_len: 6,
        }
    };

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(u8)]
    enum Node {
        File,
        ExecutableFile,
        Symlink,
        Directory,
    }

    #[test]
    fn insert() {
        let mut tree = RTree::<7, 6, 2, Node>::new();
        tree.insert(TOK_DIR, Node::Directory);
        tree.insert(TOK_SYM, Node::Symlink);
        tree.insert(TOK_FILE, Node::File);
        tree.insert(TOK_FILE_E, Node::ExecutableFile);
        pretty_assertions::assert_eq!(tree, NODE_SELECT);
    }

    #[rstest]
    #[should_panic(expected = "shared prefix")]
    #[case(&[(&b"directory"[..], true), (&b"directory"[..], true)][..])]
    #[should_panic(expected = "shared prefix")]
    #[case(&[(&b"tester"[..], true), (&b"test"[..], true)][..])]
    #[should_panic(expected = "shared prefix")]
    #[case(&[(&b"test"[..], true), (&b"tester"[..], true)][..])]
    #[should_panic(expected = "Branch factor exceeded")]
    #[case(&[(&b"file"[..], true), (&b"symlink"[..], true), (&b"dir"[..], true)][..])]
    fn insert_error(#[case] data: &[(&'static [u8], bool)]) {
        let mut tree = RTree::<7, 6, 2, bool>::new();
        for (token, value) in data {
            tree.insert(token, *value);
        }
    }

    use crate::archive::read_nar::concat_slice;
    macro_rules! prefix {
        ($slice:expr, $idx:literal) => {
            $slice.split_at($idx).0
        };
    }

    #[rstest]
    #[case::full_match(TOK_SYM, RLookup::Node(2), RMatch::Matched(TOK_SYM.len()))]
    #[case::partial_match(concat_slice!(TOK_SYM, b"more"), RLookup::Node(2), RMatch::Matched(TOK_SYM.len()))]
    #[case::mismatch_branch(concat_slice!(prefix!(TOK_SYM, 8), b"more"), RLookup::Node(3), RMatch::Mismatch(8))]
    #[case::mismatch_edge(concat_slice!(prefix!(TOK_SYM, 10), b"more"), RLookup::Edge(2, 3), RMatch::Mismatch(11))]
    #[case::needs_more_branch(&TOK_SYM[..8], RLookup::Node(3), RMatch::NeedsMore)]
    #[case::needs_more_edge(&TOK_SYM[..10], RLookup::Edge(2, 2), RMatch::NeedsMore)]
    fn lookup_test(#[case] token: &[u8], #[case] res_state: RLookup, #[case] result: RMatch) {
        let mut state = Default::default();
        assert_eq!(NODE_SELECT.lookup(&mut state, token), result);
        assert_eq!(state, res_state);
        // Matched
        // Full match TOK_DIR
        // Partial match TOK_DIR + "more"
        // Mismatched
        // Branch &TOK_SYM[..8] + "more"
        // Edge &TOK_SYM[..10] + "more"
        // Needs more
        // Branch &TOK_SYM[..8]
        // Edge &TOK_SYM[..10]
    }

    #[rstest]
    #[case(&[
            &TOK_SYM[..8],
            &TOK_SYM[8..]
        ][..], RLookup::Node(2), RMatch::Matched(TOK_SYM[8..].len()))]
    #[case(&[
            &TOK_SYM[..10],
            &TOK_SYM[10..]
        ][..], RLookup::Node(2), RMatch::Matched(TOK_SYM[10..].len()))]
    #[case(&[
            &TOK_SYM[..8],
            &TOK_SYM[8..13],
            &TOK_SYM[13..],
        ][..], RLookup::Node(2), RMatch::Matched(TOK_SYM[13..].len()))]
    fn lookup_needs_more(
        #[case] tokens: &[&[u8]],
        #[case] res_state: RLookup,
        #[case] result: RMatch,
    ) {
        let mut state = Default::default();
        for token in &tokens[..(tokens.len() - 1)] {
            assert_eq!(NODE_SELECT.lookup(&mut state, token), RMatch::NeedsMore);
        }
        assert_eq!(
            NODE_SELECT.lookup(&mut state, tokens.last().unwrap()),
            result
        );
        assert_eq!(state, res_state);
    }
}
