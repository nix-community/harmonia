use std::mem::replace;
use std::sync::Arc;

use proptest::num::sample_uniform_incl;
use proptest::prelude::Strategy;
use proptest::strategy::{NewTree, ValueTree};
use proptest::test_runner::TestRunner;

/// A **relative** `weight` of a particular `Strategy` corresponding to `T`
/// coupled with `T` itself. The weight is currently given in `u32`.
pub type W<T> = (u32, T);

/// A **relative** `weight` of a particular `Strategy` corresponding to `T`
/// coupled with `Arc<T>`. The weight is currently given in `u32`.
pub type WA<T> = (u32, Arc<T>);

/// A `Strategy` which picks from one of several delegate `Strategy`s.
#[derive(Clone, Debug)]
#[must_use = "strategies do nothing unless used"]
pub struct Union<T: Strategy> {
    options: Vec<WA<T>>,
}

impl<T: Strategy> Union<T> {
    /// Create a strategy which selects uniformly from the given delegate
    /// strategies.
    ///
    /// When shrinking, after maximal simplification of the chosen element, the
    /// strategy will move to earlier options and continue simplification with
    /// those.
    ///
    /// ## Panics
    ///
    /// Panics if `options` is empty.
    pub fn new(options: impl IntoIterator<Item = T>) -> Self {
        let options: Vec<WA<T>> = options.into_iter().map(|v| (1, Arc::new(v))).collect();
        assert!(!options.is_empty());
        Self { options }
    }

    /// Create a strategy which selects from the given delegate strategies.
    ///
    /// Each strategy is assigned a non-zero weight which determines how
    /// frequently that strategy is chosen. For example, a strategy with a
    /// weight of 2 will be chosen twice as frequently as one with a weight of
    /// 1\.
    ///
    /// ## Panics
    ///
    /// Panics if `options` is empty or any element has a weight of 0.
    ///
    /// Panics if the sum of the weights overflows a `u32`.
    pub fn new_weighted(options: Vec<W<T>>) -> Self {
        assert!(!options.is_empty());
        assert!(
            !options.iter().any(|&(w, _)| 0 == w),
            "Union option has a weight of 0"
        );
        assert!(
            options.iter().map(|&(w, _)| u64::from(w)).sum::<u64>() <= u64::from(u32::MAX),
            "Union weights overflow u32"
        );
        let options = options.into_iter().map(|(w, v)| (w, Arc::new(v))).collect();
        Self { options }
    }

    /// Add `other` as an additional alternate strategy with weight 1.
    pub fn or(mut self, other: T) -> Self {
        self.options.push((1, Arc::new(other)));
        self
    }

    pub fn shrinked(mut self, idx: usize) -> Self {
        self.options.swap(0, idx);
        self
    }
}

fn pick_weighted<I: Iterator<Item = u32>>(
    runner: &mut TestRunner,
    weights1: I,
    weights2: I,
) -> usize {
    let sum: u64 = weights1.map(u64::from).sum();
    let weighted_pick = sample_uniform_incl(runner, 0, sum - 1);
    weights2
        .scan(0u64, |state, w| {
            *state += u64::from(w);
            Some(*state)
        })
        .filter(|&v| v <= weighted_pick)
        .count()
}

impl<T: Strategy> Strategy for Union<T> {
    type Tree = UnionValueTree<T>;
    type Value = T::Value;

    fn new_tree(&self, runner: &mut TestRunner) -> NewTree<Self> {
        fn extract_weight<V>(&(w, _): &WA<V>) -> u32 {
            w
        }

        let pick_idx = pick_weighted(
            runner,
            self.options.iter().map(extract_weight::<T>),
            self.options.iter().map(extract_weight::<T>),
        );
        let pick = self.options[pick_idx].1.new_tree(runner)?;
        let shrink = if pick_idx > 0 {
            let runner = runner.clone();
            let s = Arc::clone(&self.options[0].1);
            Some((s, runner))
        } else {
            None
        };

        Ok(UnionValueTree {
            pick,
            shrink,
            prev_pick: None,
        })
    }
}

pub struct UnionValueTree<T: Strategy> {
    pick: T::Tree,
    shrink: Option<(Arc<T>, TestRunner)>,
    prev_pick: Option<T::Tree>,
}

impl<T: Strategy> ValueTree for UnionValueTree<T> {
    type Value = T::Value;

    fn current(&self) -> Self::Value {
        self.pick.current()
    }

    fn simplify(&mut self) -> bool {
        if self.pick.simplify() {
            self.prev_pick = None;
            return true;
        }
        if let Some((s, mut runner)) = self.shrink.take() {
            if let Ok(pick) = s.new_tree(&mut runner) {
                let prev_pick = replace(&mut self.pick, pick);
                self.prev_pick = Some(prev_pick);
            }
            return false;
        }
        false
    }

    fn complicate(&mut self) -> bool {
        if let Some(pick) = self.prev_pick.take() {
            self.pick = pick;
            true
        } else {
            self.pick.complicate()
        }
    }
}

impl<T: Strategy> Clone for UnionValueTree<T>
where
    T::Tree: Clone,
{
    fn clone(&self) -> Self {
        Self {
            pick: self.pick.clone(),
            shrink: self.shrink.clone(),
            prev_pick: self.prev_pick.clone(),
        }
    }
}
