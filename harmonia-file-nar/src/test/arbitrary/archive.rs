use std::collections::BTreeMap;
use std::io::Cursor;

use ::proptest::prelude::*;
use bytes::Bytes;

use crate::archive::{NarEvent, write_nar};
use crate::test::arbitrary::{arb_filename, arb_path};

#[derive(Clone, Debug)]
enum NarTree {
    File(bool, Vec<u8>),
    Symlink(String),
    Dir(BTreeMap<String, NarTree>),
}

impl NarTree {
    fn events(self, name: String, ls: &mut Vec<NarEvent<Cursor<Bytes>>>) {
        match self {
            NarTree::File(executable, contents) => {
                let size = contents.len() as u64;
                let e = NarEvent::File {
                    name: Bytes::from(name),
                    executable,
                    size,
                    reader: Cursor::new(Bytes::from(contents)),
                };
                ls.push(e);
            }
            NarTree::Symlink(target) => {
                let e = NarEvent::Symlink {
                    name: Bytes::from(name),
                    target: Bytes::from(target),
                };
                ls.push(e);
            }
            NarTree::Dir(tree) => {
                let e = NarEvent::StartDirectory {
                    name: Bytes::from(name),
                };
                ls.push(e);
                for (name, node) in tree {
                    node.events(name, ls);
                }
                let e = NarEvent::EndDirectory;
                ls.push(e);
            }
        }
    }

    fn into_events(self) -> Vec<NarEvent<Cursor<Bytes>>> {
        let mut ret = Vec::new();
        self.events(String::new(), &mut ret);
        ret
    }
}

fn arb_nar_tree(
    depth: u32,
    desired_size: u32,
    expected_branch_size: u32,
) -> impl Strategy<Value = NarTree> {
    let leaf = prop_oneof![
        (any::<bool>(), any::<Vec<u8>>()).prop_map(|(e, c)| NarTree::File(e, c)),
        arb_path().prop_map(|p| NarTree::Symlink(p.to_str().unwrap().to_owned())),
    ];
    leaf.prop_recursive(depth, desired_size, expected_branch_size, move |inner| {
        prop::collection::btree_map(arb_filename(), inner, 0..expected_branch_size as usize)
            .prop_map(NarTree::Dir)
    })
}

pub fn arb_nar_events(
    depth: u32,
    desired_size: u32,
    expected_branch_size: u32,
) -> impl Strategy<Value = Vec<NarEvent<Cursor<Bytes>>>> {
    arb_nar_tree(depth, desired_size, expected_branch_size).prop_map(|tree| tree.into_events())
}

pub fn arb_nar_contents(
    depth: u32,
    desired_size: u32,
    expected_branch_size: u32,
) -> impl Strategy<Value = Bytes> {
    arb_nar_events(depth, desired_size, expected_branch_size)
        .prop_map(|events| write_nar(events.iter()))
}
