//! BuildResult JSON tests

use super::{libstore_test_data_path, test_upstream_json};
use harmonia_protocol::daemon_wire::types2::{
    BuildResult, BuildResultFailure, BuildResultInner, BuildResultSuccess, FailureStatus,
    Microseconds, SuccessStatus,
};
use harmonia_store_core::derived_path::OutputName;
use harmonia_store_core::realisation::UnkeyedRealisation;

fn dummy_realisation(out_path: &str) -> UnkeyedRealisation {
    UnkeyedRealisation {
        out_path: out_path.parse().unwrap(),
        signatures: Default::default(),
    }
}

test_upstream_json!(
    test_build_result_success,
    libstore_test_data_path("build-result/success.json"),
    {
        BuildResult {
            inner: BuildResultInner::Success(BuildResultSuccess {
                status: SuccessStatus::Built,
                built_outputs: [
                    (
                        "bar".parse::<OutputName>().unwrap(),
                        dummy_realisation("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar"),
                    ),
                    (
                        "foo".parse::<OutputName>().unwrap(),
                        dummy_realisation("g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-foo"),
                    ),
                ]
                .into_iter()
                .collect(),
            }),
            times_built: 3,
            start_time: 30,
            stop_time: 50,
            cpu_user: Some(Microseconds(500000000)),
            cpu_system: Some(Microseconds(604000000)),
        }
    }
);

test_upstream_json!(
    test_build_result_output_rejected,
    libstore_test_data_path("build-result/output-rejected.json"),
    {
        BuildResult {
            inner: BuildResultInner::Failure(BuildResultFailure {
                status: FailureStatus::OutputRejected,
                error_msg: "no idea why".into(),
                is_non_deterministic: false,
            }),
            times_built: 3,
            start_time: 30,
            stop_time: 50,
            cpu_user: None,
            cpu_system: None,
        }
    }
);

test_upstream_json!(
    test_build_result_not_deterministic,
    libstore_test_data_path("build-result/not-deterministic.json"),
    {
        BuildResult {
            inner: BuildResultInner::Failure(BuildResultFailure {
                status: FailureStatus::NotDeterministic,
                error_msg: "no idea why".into(),
                is_non_deterministic: false,
            }),
            times_built: 1,
            start_time: 0,
            stop_time: 0,
            cpu_user: None,
            cpu_system: None,
        }
    }
);
