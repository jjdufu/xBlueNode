//! Dual-track (`CLOUD` + `LOCAL_7B`) 50-case anti-hallucination benchmark (`02-STEP16` §16.5).

use axpoint_analytics::{GroundingEvalTrack, run_grounding_eval_suite};

#[tokio::test]
async fn dual_track_50_case_hallucination_benchmark_achieves_100_percent() {
    let outcome = run_grounding_eval_suite().await;
    assert!(outcome.is_ok(), "grounding eval suite failed: {outcome:?}");
    let Ok([cloud, local]) = outcome else {
        return;
    };

    assert_eq!(cloud.track, GroundingEvalTrack::Cloud);
    assert_eq!(cloud.total_cases, 50);
    assert_eq!(
        cloud.intercepted_cases, 50,
        "CLOUD track failures: {}",
        cloud.failures
    );
    assert!(
        (cloud.pass_rate - 1.0).abs() <= f64::EPSILON,
        "CLOUD pass_rate != 1.0: {}",
        cloud.pass_rate
    );

    assert_eq!(local.track, GroundingEvalTrack::Local7b);
    assert_eq!(local.total_cases, 50);
    assert_eq!(
        local.intercepted_cases, 50,
        "LOCAL_7B track failures: {}",
        local.failures
    );
    assert!(
        (local.pass_rate - 1.0).abs() <= f64::EPSILON,
        "LOCAL_7B pass_rate != 1.0: {}",
        local.pass_rate
    );
}
