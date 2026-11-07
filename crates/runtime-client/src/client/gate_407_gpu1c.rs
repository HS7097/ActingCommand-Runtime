// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 GPU-1 C (design `rd-1031/perf-407/design-gpu1.md` §2.4, §6.2): the official OCR
//! projection reads a run's DirectML evidence in the recognition pack's two bindings, on DirectML
//! (node-level fallback, observed either way) and on the CPU after a whole-model fallback
//! (observed), and refuses every other combination that names DirectML. Fixture values only;
//! nothing runs.

use super::*;

/// One target's evidence as the observation artifact carries it: DirectML requested, resolved on
/// `resolved`, with the flags given.
fn directml_evidence(
    invocation_id: &str,
    resolved: &str,
    cpu_ep_registered: bool,
    fallback_forbidden: bool,
    fallback_observed: Value,
) -> Value {
    json!({
        "invocation_id": invocation_id,
        "session_id": "session.fixture",
        "session_generation": 1,
        "requested_provider": "directml",
        "resolved_provider": resolved,
        "requested_cuda_ordinal": null,
        "requested_cuda_identity": null,
        "resolved_cuda_ordinal": null,
        "resolved_cuda_identity": null,
        "provider_implementation": "fixture.ocr",
        "provider_binary_sha256": "1".repeat(64),
        "runtime_version": "fixture-runtime-1",
        "model_ref": "fixture-model",
        "model_sha256": "2".repeat(64),
        "cpu_ep_registered": cpu_ep_registered,
        "cpu_fallback_disabled": false,
        "fallback_forbidden": fallback_forbidden,
        "fallback_observed": fallback_observed,
        "complete": true
    })
}

fn on_directml(invocation_id: &str, observed: bool) -> Value {
    directml_evidence(invocation_id, "directml", false, false, json!(observed))
}

fn on_cpu_after_fallback(invocation_id: &str) -> Value {
    directml_evidence(invocation_id, "cpu", true, false, json!(true))
}

fn target(target_id: &str, execution: Value) -> Value {
    json!({
        "target_id": target_id,
        "text": "fixture",
        "confidence": 0.9,
        "blocks": [],
        "execution": execution
    })
}

#[test]
fn official_ocr_reads_directml_and_whole_model_fallback_evidence() {
    let mut records = parse_official_ocr_observation(
        0,
        &json!({"targets": [
            target("screen.left", on_directml("invocation.0.left", true)),
            target("screen.right", on_directml("invocation.0.right", false)),
            target("screen.fallback", on_cpu_after_fallback("invocation.0.fallback")),
        ]}),
    )
    .expect("DirectML and whole-model fallback evidence is read");
    records.extend(
        parse_official_ocr_observation(
            1,
            &target("screen.left", on_directml("invocation.1.left", true)),
        )
        .expect("a direct observation on DirectML"),
    );
    // Workflow #407 GPU-1 C fix 1 (PR #1535): `fallback_observed` is per call and not part of
    // the binding, so the two DirectML targets of one session, observed true and false, share
    // one binding.
    let executions = project_official_ocr_providers(records).expect("one binding per target");
    let projected = executions
        .iter()
        .map(|execution| {
            (
                execution.model_ref.as_str(),
                execution.requested_provider,
                execution.actual_provider,
                execution.cpu_ep_registered,
                execution.fallback_forbidden,
                execution.fallback_observed,
                execution.strict_no_fallback,
                execution.evidence.len(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        projected,
        [
            (
                "fixture-model",
                RuntimeOfficialOcrProviderKind::Directml,
                RuntimeOfficialOcrProviderKind::Cpu,
                true,
                false,
                Some(true),
                false,
                1,
            ),
            (
                "fixture-model",
                RuntimeOfficialOcrProviderKind::Directml,
                RuntimeOfficialOcrProviderKind::Directml,
                false,
                false,
                Some(true),
                false,
                3,
            ),
        ]
    );
    let stdout = serde_json::to_value(&executions).expect("serialize the provider executions");
    assert_eq!(stdout[0]["requested_provider"], "directml");
    assert_eq!(stdout[0]["actual_provider"], "cpu");
    assert_eq!(stdout[1]["actual_provider"], "directml");

    for (label, execution) in [
        (
            "DirectML without an observation",
            directml_evidence("invocation.bad", "directml", false, false, Value::Null),
        ),
        (
            "DirectML with fallback forbidden",
            directml_evidence("invocation.bad", "directml", false, true, json!(true)),
        ),
        (
            "DirectML with the CPU EP registered",
            directml_evidence("invocation.bad", "directml", true, false, json!(true)),
        ),
        (
            "whole-model fallback not observed",
            directml_evidence("invocation.bad", "cpu", true, false, json!(false)),
        ),
        (
            "whole-model fallback without the CPU EP",
            directml_evidence("invocation.bad", "cpu", false, false, json!(true)),
        ),
        (
            "whole-model fallback forbidden",
            directml_evidence("invocation.bad", "cpu", true, true, json!(true)),
        ),
        (
            "DirectML resolved on CUDA",
            directml_evidence("invocation.bad", "cuda", false, false, json!(true)),
        ),
        ("a CPU request resolved on DirectML", {
            let mut execution = on_directml("invocation.bad", true);
            execution["requested_provider"] = json!("cpu");
            execution
        }),
        ("a CPU request with DirectML's fallback", {
            let mut execution = on_cpu_after_fallback("invocation.bad");
            execution["requested_provider"] = json!("cpu");
            execution
        }),
    ] {
        let error =
            parse_official_ocr_observation(0, &target("screen.left", execution)).expect_err(label);
        assert_eq!(
            error.code().as_str(),
            ClientCode::RuntimeOfficialOcrProviderEvidenceInvalid.as_str(),
            "{label}"
        );
    }
}
