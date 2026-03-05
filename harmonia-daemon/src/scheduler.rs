// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Internal build scheduler for standalone daemon mode.
//!
//! Manages concurrent builds with a `max_jobs` semaphore and DAG-aware
//! dependency resolution. Builds are scheduled FIFO within each wave
//! of the topological sort.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore};

use harmonia_store_core::store_path::StorePath;

/// Result of a scheduled build.
#[derive(Debug, Clone)]
pub enum ScheduledBuildResult {
    /// Build completed successfully.
    Success,
    /// Build failed.
    Failed(String),
    /// Build was not attempted because a dependency failed.
    DependencyFailed(StorePath),
}

/// A node in the build dependency graph.
#[derive(Debug, Clone)]
pub struct BuildNode {
    /// The derivation path to build.
    pub drv_path: StorePath,
    /// Paths this build depends on (must be built first).
    pub dependencies: BTreeSet<StorePath>,
}

/// Internal build scheduler.
///
/// Enforces `max_jobs` concurrent builds and resolves build order
/// based on the dependency DAG.
pub struct BuildScheduler {
    max_jobs: Arc<Semaphore>,
    max_jobs_count: usize,
}

impl BuildScheduler {
    /// Create a new scheduler with the given max concurrent jobs.
    pub fn new(max_jobs: usize) -> Self {
        Self {
            max_jobs: Arc::new(Semaphore::new(max_jobs)),
            max_jobs_count: max_jobs,
        }
    }

    /// Schedule and execute builds for a DAG of derivations.
    ///
    /// The `build_fn` closure is called for each derivation that needs building.
    /// Returns a map of drv_path → result for all nodes.
    pub async fn build_dag<F, Fut>(
        &self,
        nodes: Vec<BuildNode>,
        build_fn: F,
    ) -> BTreeMap<StorePath, ScheduledBuildResult>
    where
        F: Fn(StorePath) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let results: Arc<Mutex<BTreeMap<StorePath, ScheduledBuildResult>>> =
            Arc::new(Mutex::new(BTreeMap::new()));

        // Build a dependency map for quick lookup
        let dep_map: HashMap<StorePath, BTreeSet<StorePath>> = nodes
            .iter()
            .map(|n| (n.drv_path.clone(), n.dependencies.clone()))
            .collect();
        let all_paths: BTreeSet<StorePath> = nodes.iter().map(|n| n.drv_path.clone()).collect();

        // Topological sort: process in waves
        let mut remaining: BTreeSet<StorePath> = all_paths.clone();
        let build_fn = Arc::new(build_fn);

        while !remaining.is_empty() {
            // Find all nodes whose dependencies are satisfied
            let ready: Vec<StorePath> = remaining
                .iter()
                .filter(|path| {
                    dep_map
                        .get(*path)
                        .map(|deps| {
                            deps.iter()
                                .all(|dep| !remaining.contains(dep) || !all_paths.contains(dep))
                        })
                        .unwrap_or(true)
                })
                .cloned()
                .collect();

            if ready.is_empty() && !remaining.is_empty() {
                // Cycle detected or unresolvable deps — fail remaining
                for path in &remaining {
                    results.lock().await.insert(
                        path.clone(),
                        ScheduledBuildResult::Failed("unresolvable dependency cycle".into()),
                    );
                }
                break;
            }

            // Check if any dependency has failed — propagate DependencyFailed
            let mut actually_ready = Vec::new();
            for path in &ready {
                let deps = dep_map.get(path).cloned().unwrap_or_default();
                let results_guard = results.lock().await;
                let failed_dep = deps.iter().find(|dep| {
                    matches!(
                        results_guard.get(*dep),
                        Some(ScheduledBuildResult::Failed(_))
                            | Some(ScheduledBuildResult::DependencyFailed(_))
                    )
                });

                if let Some(failed) = failed_dep {
                    drop(results_guard);
                    results.lock().await.insert(
                        path.clone(),
                        ScheduledBuildResult::DependencyFailed(failed.clone()),
                    );
                } else {
                    drop(results_guard);
                    actually_ready.push(path.clone());
                }
            }

            // Remove ready nodes from remaining
            for path in &ready {
                remaining.remove(path);
            }

            // Spawn builds for actually ready nodes, limited by semaphore
            let mut handles = Vec::new();
            for path in actually_ready {
                let semaphore = self.max_jobs.clone();
                let build_fn = build_fn.clone();
                let results = results.clone();
                let path_clone = path.clone();

                let handle = tokio::spawn(async move {
                    let _permit = semaphore.acquire().await.unwrap();
                    let result = match build_fn(path_clone.clone()).await {
                        Ok(()) => ScheduledBuildResult::Success,
                        Err(msg) => ScheduledBuildResult::Failed(msg),
                    };
                    results.lock().await.insert(path_clone, result);
                });
                handles.push(handle);
            }

            // Wait for this wave to complete
            for handle in handles {
                let _ = handle.await;
            }
        }

        Arc::try_unwrap(results)
            .unwrap_or_else(|arc| {
                let guard = arc.blocking_lock();
                Mutex::new(guard.clone())
            })
            .into_inner()
    }

    /// Number of concurrent build slots.
    pub fn max_jobs(&self) -> usize {
        self.max_jobs_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// With `max_jobs = 2`, submit 3 builds concurrently → only 2 run at a time.
    #[tokio::test]
    async fn test_max_jobs_concurrency() {
        let scheduler = BuildScheduler::new(2);

        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        let nodes = vec![
            BuildNode {
                drv_path: StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-a.drv")
                    .unwrap(),
                dependencies: BTreeSet::new(),
            },
            BuildNode {
                drv_path: StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-b.drv")
                    .unwrap(),
                dependencies: BTreeSet::new(),
            },
            BuildNode {
                drv_path: StorePath::from_base_path("cccccccccccccccccccccccccccccccc-c.drv")
                    .unwrap(),
                dependencies: BTreeSet::new(),
            },
        ];

        let concurrent_clone = concurrent.clone();
        let max_concurrent_clone = max_concurrent.clone();

        let results = scheduler
            .build_dag(nodes, move |_path| {
                let concurrent = concurrent_clone.clone();
                let max_concurrent = max_concurrent_clone.clone();
                async move {
                    let cur = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                    max_concurrent.fetch_max(cur, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    concurrent.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                }
            })
            .await;

        // All should succeed
        assert_eq!(results.len(), 3);
        for result in results.values() {
            assert!(matches!(result, ScheduledBuildResult::Success));
        }

        // Max concurrency should be <= 2
        assert!(
            max_concurrent.load(Ordering::SeqCst) <= 2,
            "Max concurrent should be <= 2, got {}",
            max_concurrent.load(Ordering::SeqCst)
        );
    }

    /// Diamond DAG (A←B,C←D) → A builds first, B and C next, D last.
    #[tokio::test]
    async fn test_diamond_dag() {
        let scheduler = BuildScheduler::new(4);

        let path_a = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-a.drv").unwrap();
        let path_b = StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-b.drv").unwrap();
        let path_c = StorePath::from_base_path("cccccccccccccccccccccccccccccccc-c.drv").unwrap();
        let path_d = StorePath::from_base_path("dddddddddddddddddddddddddddddddd-d.drv").unwrap();

        let build_order = Arc::new(Mutex::new(Vec::new()));

        let nodes = vec![
            BuildNode {
                drv_path: path_a.clone(),
                dependencies: BTreeSet::new(),
            },
            BuildNode {
                drv_path: path_b.clone(),
                dependencies: BTreeSet::from([path_a.clone()]),
            },
            BuildNode {
                drv_path: path_c.clone(),
                dependencies: BTreeSet::from([path_a.clone()]),
            },
            BuildNode {
                drv_path: path_d.clone(),
                dependencies: BTreeSet::from([path_b.clone(), path_c.clone()]),
            },
        ];

        let order = build_order.clone();
        let results = scheduler
            .build_dag(nodes, move |path| {
                let order = order.clone();
                async move {
                    order.lock().await.push(path);
                    Ok(())
                }
            })
            .await;

        assert_eq!(results.len(), 4);
        for result in results.values() {
            assert!(matches!(result, ScheduledBuildResult::Success));
        }

        let order = build_order.lock().await;
        // A must be before B and C
        let a_idx = order.iter().position(|p| *p == path_a).unwrap();
        let b_idx = order.iter().position(|p| *p == path_b).unwrap();
        let c_idx = order.iter().position(|p| *p == path_c).unwrap();
        let d_idx = order.iter().position(|p| *p == path_d).unwrap();

        assert!(a_idx < b_idx, "A should build before B");
        assert!(a_idx < c_idx, "A should build before C");
        assert!(b_idx < d_idx, "B should build before D");
        assert!(c_idx < d_idx, "C should build before D");
    }

    /// Dependency build failure → all transitive dependents get DependencyFailed.
    #[tokio::test]
    async fn test_dependency_failure_propagation() {
        let scheduler = BuildScheduler::new(4);

        let path_a = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-a.drv").unwrap();
        let path_b = StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-b.drv").unwrap();
        let path_c = StorePath::from_base_path("cccccccccccccccccccccccccccccccc-c.drv").unwrap();

        let nodes = vec![
            BuildNode {
                drv_path: path_a.clone(),
                dependencies: BTreeSet::new(),
            },
            BuildNode {
                drv_path: path_b.clone(),
                dependencies: BTreeSet::from([path_a.clone()]),
            },
            BuildNode {
                drv_path: path_c.clone(),
                dependencies: BTreeSet::from([path_b.clone()]),
            },
        ];

        let fail_path = path_a.clone();
        let results = scheduler
            .build_dag(nodes, move |path| {
                let fail_path = fail_path.clone();
                async move {
                    if path == fail_path {
                        Err("build failed".to_string())
                    } else {
                        Ok(())
                    }
                }
            })
            .await;

        // A should fail
        assert!(matches!(
            results.get(&path_a),
            Some(ScheduledBuildResult::Failed(_))
        ));

        // B should get DependencyFailed (depends on A)
        assert!(
            matches!(
                results.get(&path_b),
                Some(ScheduledBuildResult::DependencyFailed(_))
            ),
            "B should get DependencyFailed, got: {:?}",
            results.get(&path_b)
        );

        // C should get DependencyFailed (depends on B which depends on A)
        assert!(
            matches!(
                results.get(&path_c),
                Some(ScheduledBuildResult::DependencyFailed(_))
            ),
            "C should get DependencyFailed, got: {:?}",
            results.get(&path_c)
        );
    }
}
