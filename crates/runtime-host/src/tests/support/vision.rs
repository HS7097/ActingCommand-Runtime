// SPDX-License-Identifier: AGPL-3.0-only

#[derive(Debug, Default)]
struct FakeVisionProvider {
    ocr_calls: AtomicU64,
    ocr_started: AtomicBool,
    block_ocr: AtomicBool,
    ocr_failure_detail: Option<&'static str>,
    raw_evidence: bool,
    nn_calls: AtomicU64,
}

impl VisionProvider for FakeVisionProvider {
    fn require_ocr_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        if model_ref == "PP-OCRv6_medium"
            && model_sha256 == "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        {
            Ok(())
        } else {
            Err(VisionProviderError::new(
                VisionProviderErrorCode::ModelMismatch,
                "unexpected OCR model identity",
            ))
        }
    }

    fn require_nn_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        if self.raw_evidence
            && model_ref == "neutral-classifier"
            && model_sha256 == "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        {
            return Ok(());
        }
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "NN capability is unavailable",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        self.ocr_calls.fetch_add(1, Ordering::AcqRel);
        self.ocr_started.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.block_ocr.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return Err(VisionProviderError::new(
                    VisionProviderErrorCode::Timeout,
                    "fixture OCR gate exceeded deadline",
                ));
            }
            thread::sleep(Duration::from_millis(1));
        }
        if let Some(detail) = self.ocr_failure_detail {
            return Err(VisionProviderError::new(
                VisionProviderErrorCode::Internal,
                detail,
            ));
        }
        let text = match request.frame.rgb8_pixels.get(..3) {
            Some([255, 0, 0]) => "home",
            Some([0, 0, 255]) => "terminal",
            Some([255, 255, 0]) => "error",
            _ => "unknown",
        };
        if self.raw_evidence {
            use actingcommand_recognition_pack::{OcrProviderTextBlock, PackRect};
            assert_eq!(
                request.region,
                PackRect {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 1
                }
            );
            return Ok(OcrProviderResult {
                text: format!("provider aggregate {text}"),
                confidence: Some(0.99),
                blocks: vec![
                    OcrProviderTextBlock {
                        text: "marker".into(),
                        rect: PackRect {
                            x: 1,
                            y: 0,
                            width: 1,
                            height: 1,
                        },
                        confidence: Some(0.75),
                    },
                    OcrProviderTextBlock {
                        text: text.into(),
                        rect: PackRect {
                            x: 0,
                            y: 0,
                            width: 1,
                            height: 1,
                        },
                        confidence: Some(0.99),
                    },
                ],
            });
        }
        Ok(OcrProviderResult {
            text: text.to_owned(),
            blocks: Vec::new(),
            confidence: Some(0.99),
        })
    }

    fn classify(
        &self,
        request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        if self.raw_evidence {
            use actingcommand_recognition_pack::{NnProviderLabel, PackRect};
            self.nn_calls.fetch_add(1, Ordering::AcqRel);
            assert_eq!(
                request.region,
                PackRect {
                    x: 1,
                    y: 0,
                    width: 1,
                    height: 1
                }
            );
            let mut labels = (0..1023)
                .map(|index| NnProviderLabel {
                    label: format!("{index:04}{}", "x".repeat(4092)),
                    score: 0.25,
                })
                .collect::<Vec<_>>();
            labels.push(NnProviderLabel {
                label: "ready".into(),
                score: 0.98,
            });
            return Ok(NnProviderResult { labels });
        }
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "NN capability is unavailable",
        ))
    }
}
