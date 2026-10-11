// SPDX-License-Identifier: AGPL-3.0-only

//! G15, inventory key K15: the candidate-set hash (`contracts/candidate-projection.md`,
//! "Candidate-set hash"). It covers every candidate's geometry and every feature's type and
//! value, and never `confidence` or the frame's identity.

use actingcommand_contract::{
    CandidateFeature, CandidateFeatureMap, CandidateFrame, CandidateLayoutKind,
    CandidateProjection, CandidateRecognitionEvidence, CandidateRecognitionInput,
    CandidateRecognitionSample, CandidateRect, ProjectedCandidate,
};

const LAYOUT_ID: &str = "slots/main";

fn candidate() -> ProjectedCandidate {
    ProjectedCandidate {
        id: format!("{LAYOUT_ID}#00"),
        instance_index: 0,
        actionable: true,
        rect: CandidateRect {
            x: 214,
            y: 204,
            width: 176,
            height: 90,
        },
        click: CandidateRect {
            x: 224,
            y: 214,
            width: 156,
            height: 60,
        },
        features: CandidateFeatureMap::from([
            (
                "mark_score".to_owned(),
                CandidateFeature::Integer {
                    value: 991,
                    confidence: Some(870),
                },
            ),
            (
                "marked".to_owned(),
                CandidateFeature::Boolean {
                    value: true,
                    confidence: Some(940),
                },
            ),
        ]),
    }
}

fn projection(candidate: ProjectedCandidate) -> CandidateProjection {
    CandidateProjection::new(
        "list_page",
        LAYOUT_ID,
        CandidateLayoutKind::FixedSlots,
        CandidateFrame {
            width: 1280,
            height: 720,
        },
        vec![candidate],
    )
    .expect("a valid candidate projection")
}

/// The recognition evidence of `mark_score`, read on the frame with this index and hash.
fn evidence_on_frame(frame_index: u8, frame_rgb8_sha256: &str) -> CandidateRecognitionEvidence {
    CandidateRecognitionEvidence {
        candidate_id: format!("{LAYOUT_ID}#00"),
        feature: "mark_score".to_owned(),
        samples: vec![CandidateRecognitionSample {
            frame_index,
            frame_rgb8_sha256: frame_rgb8_sha256.to_owned(),
            input: CandidateRecognitionInput::Scalar {
                value: Some(CandidateFeature::Integer {
                    value: 991,
                    confidence: None,
                }),
            },
            value: CandidateFeature::Integer {
                value: 991,
                confidence: Some(870),
            },
        }],
    }
}

#[test]
fn gate_candidate_hash_ignores_confidence_and_frame_identity() {
    let base = projection(candidate());

    let mut other_confidence = candidate();
    other_confidence.features.insert(
        "mark_score".to_owned(),
        CandidateFeature::Integer {
            value: 991,
            confidence: Some(512),
        },
    );
    assert_eq!(
        projection(other_confidence).candidate_set_sha256(),
        base.candidate_set_sha256(),
        "a feature's confidence"
    );

    let first_frame = projection(candidate())
        .with_recognition_evidence(vec![evidence_on_frame(
            0,
            "e743b4f8b446f945137ba1ea6042627fdee2e059334b506a848517acf12ec4a4",
        )])
        .expect("evidence of the first frame");
    let second_frame = projection(candidate())
        .with_recognition_evidence(vec![evidence_on_frame(
            1,
            "993a15cb20ff3284a64437568caa5581aa1a26791bf5de1c8f18855a2e32ad95",
        )])
        .expect("evidence of the second frame");
    assert_eq!(
        first_frame
            .compute_candidate_set_sha256()
            .expect("hash the first frame's projection"),
        second_frame
            .compute_candidate_set_sha256()
            .expect("hash the second frame's projection"),
        "the frame's identity"
    );
}

#[test]
fn gate_candidate_hash_covers_geometry_and_feature_values() {
    let base = projection(candidate());
    let changes: [(&str, fn(&mut ProjectedCandidate)); 10] = [
        ("rect.x", |candidate: &mut ProjectedCandidate| {
            candidate.rect.x += 1
        }),
        ("rect.y", |candidate: &mut ProjectedCandidate| {
            candidate.rect.y += 1
        }),
        ("rect.width", |candidate: &mut ProjectedCandidate| {
            candidate.rect.width += 1
        }),
        ("rect.height", |candidate: &mut ProjectedCandidate| {
            candidate.rect.height += 1
        }),
        ("click.x", |candidate: &mut ProjectedCandidate| {
            candidate.click.x += 1
        }),
        ("click.y", |candidate: &mut ProjectedCandidate| {
            candidate.click.y += 1
        }),
        ("click.width", |candidate: &mut ProjectedCandidate| {
            candidate.click.width += 1
        }),
        ("click.height", |candidate: &mut ProjectedCandidate| {
            candidate.click.height += 1
        }),
        (
            "an integer feature's value",
            |candidate: &mut ProjectedCandidate| {
                candidate.features.insert(
                    "mark_score".to_owned(),
                    CandidateFeature::Integer {
                        value: 992,
                        confidence: Some(870),
                    },
                );
            },
        ),
        (
            "a boolean feature's value",
            |candidate: &mut ProjectedCandidate| {
                candidate.features.insert(
                    "marked".to_owned(),
                    CandidateFeature::Boolean {
                        value: false,
                        confidence: Some(940),
                    },
                );
            },
        ),
    ];
    for (field, change) in changes {
        let mut changed = candidate();
        change(&mut changed);
        assert_ne!(
            projection(changed).candidate_set_sha256(),
            base.candidate_set_sha256(),
            "{field}"
        );
    }
}
