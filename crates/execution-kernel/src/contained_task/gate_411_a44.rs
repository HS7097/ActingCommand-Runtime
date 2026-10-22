// SPDX-License-Identifier: AGPL-3.0-only

// #411 A44 writer (0.12.x; ledger revision 4 row LR-8, register §1.153 and §1.173; exec keys 171
// and 172): a page recognition hands the host each candidate page's verdict, the trace member
// `candidate_scores` of `RecognitionCompleted`, which the host writes into
// `task.recognition_completed` unchanged.
//
// - T1 (exec 171): three candidate pages, the third matches: three entries in candidate order, the
//   first two not passed, each with its best score and its first failed target, the third passed.
// - T2 (exec 172): 18 candidate pages, none matches: the first 16 are listed, none passed, and
//   `omitted` counts the other 2.
//
// Each package is a synthetic `neutral` package admitted through
// `PreparedContainedTask::load_with_vision_provider`. Every target of a page is an OCR read of its
// own one-pixel cell, the cells numbered in the order the pages declare their targets; the
// scripted provider reads `ok` in every cell at the confidence the test scripts for it, and a read
// passes at a confidence of 0.5 or more, so a target's score is its scripted confidence in milli.
// The confidences are exact binary fractions. Neither package has a required entry (its single
// entry page is no home page), so the run's first recognition evaluates every page of the
// package, in the page set's order. The runs play scripted frames on the scripted clock (EXEC-I4).

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

// Reads `ok` in every cell, at the confidence `confidences[x]` of the cell at column `x`.
#[derive(Debug)]
struct ScriptedConfidence {
    confidences: Vec<f32>,
}

impl VisionProvider for ScriptedConfidence {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let cell = usize::try_from(request.region.x).expect("a cell on the frame");
        let confidence = self.confidences[cell];
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: "ok".to_owned(),
            blocks: vec![OcrProviderTextBlock {
                text: "ok".to_owned(),
                rect: request.region,
                confidence: Some(confidence),
            }],
            confidence: Some(confidence),
            detection: None,
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// A page of the package: its id and the targets it requires, in declared order.
struct Page {
    id: String,
    required: Vec<String>,
}

fn page(id: &str, required: &[&str]) -> Page {
    Page {
        id: id.to_owned(),
        required: required.iter().map(|target| (*target).to_owned()).collect(),
    }
}

// The package of `pages`: target `n` of the package reads cell `n` (a frame `cells` wide), its
// entry page the first page, its target page the last, with one tap from the one to the other.
fn admitted(case: &str, pages: &[Page], confidences: Vec<f32>) -> PreparedContainedTask {
    let cells = pages.iter().map(|page| page.required.len()).sum::<usize>();
    assert_eq!(confidences.len(), cells, "{case}: one confidence per cell");
    let targets = pages
        .iter()
        .flat_map(|page| page.required.iter())
        .enumerate()
        .map(|(cell, target)| {
            json!({
                "type": "ocr", "id": target,
                "region": {"x": cell, "y": 0, "width": 1, "height": 1},
                "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact",
                "expected": ["ok"], "case_sensitive": false, "minimum_confidence": 0.5,
                "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
            })
        })
        .collect::<Vec<_>>();
    let first = &pages.first().expect("a page").id;
    let last = &pages.last().expect("a page").id;
    let files: [(&str, Value); 5] = [
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v2",
                "package_id": "neutral.a44.task",
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": cells, "height": 1},
                "entry_task_id": "task",
                "capture_interval_ms": 100,
                "step_timeout_ms": 300,
                "timeout_ms": 60_000
            }),
        ),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        (
            "resources/operations/task/task.json",
            json!({
                "schema_version": "0.9",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "coordinate_space": {"width": cells, "height": 1},
                "entry_page": first,
                "target_page": last,
                "operations": [{
                    "id": "op_a", "from": first, "to": last,
                    "click": {"kind": "point", "x": 0, "y": 0},
                    "unguarded_trusted_coordinate": true, "retryable": false
                }]
            }),
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.7",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": cells, "height": 1},
                "defaults": {"color_max_distance": 0.0},
                "targets": targets
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({
                "schema_version": "0.3",
                "pages": pages
                    .iter()
                    .map(|page| json!({
                        "id": format!("neutral/{}", page.id),
                        "required": page.required,
                        "optional": [],
                        "forbidden": []
                    }))
                    .collect::<Vec<_>>()
            }),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in files {
        zip.start_file(path, options).expect("zip entry");
        serde_json::to_writer(&mut zip, &value).expect("zip JSON");
        zip.write_all(b"\n").expect("zip newline");
    }
    let bytes = zip.finish().expect("finish zip").into_inner();
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &bytes,
        Arc::new(ScriptedConfidence { confidences }),
    )
    .unwrap_or_else(|error| panic!("{case}: admission refused the package: {error}"))
}

// Runs `task` on a device that always shows one blank frame `cells` wide (the provider decides
// every read) and returns the run's first recognition: its candidate pages, its page and the
// verdicts it hands the host.
fn first_recognition(
    task: &PreparedContainedTask,
    cells: u32,
) -> (Vec<String>, Option<String>, RecognitionCandidateScores) {
    let pixels = vec![[0_u8, 0, 0]; usize::try_from(cells).expect("cells")];
    let mut runtime = ScriptedRuntime::new(vec![vec![ScriptedFrame::rgb(cells, 1, &pixels)]]);
    // The run's own end is not this slice's: the test reads what its first recognition recorded.
    let _outcome = task.run(&mut runtime);
    runtime
        .traces()
        .iter()
        .find_map(|trace| match trace {
            ContainedTaskTrace::RecognitionCompleted {
                candidate_pages,
                page_label,
                candidate_scores,
                ..
            } => Some((
                candidate_pages.clone(),
                page_label.clone(),
                (**candidate_scores).clone(),
            )),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the run recorded no recognition: {:?}", runtime.traces()))
}

fn verdict(
    page_id: &str,
    passed: bool,
    best_score_milli: u32,
    first_failed_target_id: Option<&str>,
) -> RecognitionCandidateScore {
    RecognitionCandidateScore {
        page_id: page_id.to_owned(),
        passed,
        best_score_milli: Some(best_score_milli),
        first_failed_target_id: first_failed_target_id.map(str::to_owned),
    }
}

// T1 (exec 171): `page_a` requires `a/top` (0.25, fails) and `a/bottom` (0.75); `page_b` requires
// `b/top` (0.875) and `b/bottom` (0.375, fails); `page_c` requires `c/top` (0.9375) and `c/bottom`
// (0.625), so only `page_c` matches. The recognition hands over three entries in candidate order:
// `page_a` not passed, best 750, failed at `a/top`; `page_b` not passed, best 875, failed at
// `b/bottom`; `page_c` passed with best 937 and no failed target; nothing omitted.
#[test]
fn a44_t1_every_candidate_page_has_its_best_score_and_verdict_in_candidate_order() {
    let pages = [
        page("page_a", &["a/top", "a/bottom"]),
        page("page_b", &["b/top", "b/bottom"]),
        page("page_c", &["c/top", "c/bottom"]),
    ];
    let task = admitted(
        "the third page matches",
        &pages,
        vec![0.25, 0.75, 0.875, 0.375, 0.9375, 0.625],
    );

    let (candidate_pages, page_label, candidate_scores) = first_recognition(&task, 6);

    assert_eq!(
        candidate_pages,
        ["neutral/page_a", "neutral/page_b", "neutral/page_c"]
    );
    assert_eq!(page_label.as_deref(), Some("neutral/page_c"));
    assert_eq!(
        candidate_scores,
        RecognitionCandidateScores {
            pages: vec![
                verdict("neutral/page_a", false, 750, Some("a/top")),
                verdict("neutral/page_b", false, 875, Some("b/bottom")),
                verdict("neutral/page_c", true, 937, None),
            ],
            omitted: 0,
        }
    );
}

// T2 (exec 172): 18 pages `page_00` to `page_17`, page `n` requiring `cell/n`, read at 0.125,
// 0.25 or 0.375 (`n` mod 3), so no page matches. The recognition hands over the first 16 candidate
// pages, `TASK_RECOGNITION_CANDIDATE_SCORE_LIMIT`, in candidate order, none passed, each with its
// score and its one target as the failed one, and `omitted` 2.
#[test]
fn a44_t2_a_recognition_lists_the_first_16_candidate_pages_and_counts_the_rest() {
    let ids = (0..18).map(|n| format!("{n:02}")).collect::<Vec<_>>();
    let pages = ids
        .iter()
        .map(|n| page(&format!("page_{n}"), &[format!("cell/{n}").as_str()]))
        .collect::<Vec<_>>();
    let score = |n: usize| 125 * (u32::try_from(n % 3).expect("n mod 3") + 1);
    let confidences = (0..18)
        .map(|n| score(n) as f32 / 1000.0)
        .collect::<Vec<_>>();
    let task = admitted("no page matches", &pages, confidences);

    let (candidate_pages, page_label, candidate_scores) = first_recognition(&task, 18);

    assert_eq!(candidate_pages.len(), 18);
    assert_eq!(page_label, None);
    assert_eq!(TASK_RECOGNITION_CANDIDATE_SCORE_LIMIT, 16);
    assert_eq!(
        candidate_scores,
        RecognitionCandidateScores {
            pages: ids
                .iter()
                .take(16)
                .enumerate()
                .map(|(index, n)| verdict(
                    &format!("neutral/page_{n}"),
                    false,
                    score(index),
                    Some(format!("cell/{n}").as_str()),
                ))
                .collect(),
            omitted: 2,
        }
    );
}
