# PPOCR recognizer input

`frame_region_to_recognition_tensor` prepares the recognizer input for both direct
ROI recognition and full-frame detection followed by per-box recognition. It reads
the original frame inside the selected rectangle. RGB8 remains RGB at the external
Frame/Artifact boundary; RGBA8 ignores alpha and Gray8 repeats its intensity.

The recognizer receives float32 NCHW in **BGR** order. The bound model's
[fixed configuration](https://huggingface.co/PaddlePaddle/PP-OCRv6_medium_rec_onnx/blob/50c7eacafc52fa7bcf4194e8cd08e46f8558504b/inference.yml)
selects BGR decoding and `RecResizeImg`. Its `inference.onnx` LFS SHA-256 is
`9c09abf0957f7968c7586464b7397b84ad2387a0497a351af40e9acc71b673ba`.
The pinned [PaddleOCR implementation](https://github.com/PaddlePaddle/PaddleOCR/blob/db4b14b6bde5d4cbd3cd3e62906e577dad358c21/ppocr/data/imaug/rec_img_aug.py#L631)
resizes uint8 pixels with `cv2.resize`'s linear interpolation before normalization.

The local implementation follows the published CPU uint8 `INTER_LINEAR` arithmetic
in [OpenCV 4.12.0](https://github.com/opencv/opencv/blob/4.12.0/modules/imgproc/src/resize.cpp):
half-pixel coordinates, replicated rectangle edges, 11-bit nearest-even coefficients,
horizontal integer accumulation, and the staged vertical shifts in
`VResizeLinear<uchar, int, short, ...>`. Vertical edge weights remain fractional
while source rows clamp. Quantization produces a uint8 sample before the existing
`value / 127.5 - 1` normalization. This defines the local arithmetic; optimized
OpenCV backends are not an assertion of bitwise equivalence.

Model input validation and shape selection retain their existing rules: positive
model dimensions, dynamic height 48, and dynamic width clamped to 32–320. Content
width is `ceil(region.width / region.height * input.height)`, capped at the tensor
width. Unused right columns are float32 zero. Invalid rectangles and unavailable
pixel bytes return the existing errors. Detection preprocessing, frame coordinates,
session ownership, model/dictionary bindings and CTC decoding keep their contracts.

The workspace CI runs the existing Provider specifications, including neutral color,
fractional scaling, integer quantization, rectangle bounds and zero-padding cases.
Recognition accuracy is established separately through formal Provider results on
saved artifacts; tensor specifications alone do not establish that outcome.
