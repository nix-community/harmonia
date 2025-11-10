pub mod arbitrary;
pub mod derived_path;

#[macro_export]
macro_rules! btree_map {
    () => { BTreeMap::new() };
    ($($k:expr => $v:expr),+ $(,)?) => {{
        let mut ret = std::collections::BTreeMap::new();
        $(
            ret.insert($k.parse().unwrap(), $v.parse().unwrap());
        )+
        ret
    }};
}

#[macro_export]
macro_rules! btree_set {
    () => { BTreeSet::new() };
    ($($v:expr),+ $(,)?) => {{
        let mut ret = std::collections::BTreeSet::new();
        $(
            ret.insert($v.parse().unwrap());
        )+
        ret
    }};
}
