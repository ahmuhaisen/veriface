"""What each shipped model is, and the exact I/O contract the Rust side implements.

Facts here are taken from MediaPipe's own sources at ``upstream.MP_LEGACY_COMMIT``:
``tasks/cc/vision/face_detector/face_detector_graph.cc``,
``tasks/cc/vision/face_landmarker/{face_landmarks_detector,face_blendshapes}_graph.cc``,
``calculators/tensor/image_to_tensor_converter_opencv.cc`` and ``image_to_tensor_utils.cc``,
plus the TFLite metadata embedded in each model (normalisation, colour space, output names).
"""

from __future__ import annotations

from dataclasses import dataclass, field

MP_LICENCE_URL = "https://github.com/google-ai-edge/mediapipe/blob/master/LICENSE"
MP_CARDS = "https://storage.googleapis.com/mediapipe-assets/"
SCOPE_CLAUSE = (
    "Model card lists as out of scope: 'Any form of surveillance or identity recognition is explicitly out of scope "
    "and not enabled by this technology.' A statement of intended scope, not a licence term; our use (detection in "
    "a 1:1 verification pipeline) needs legal review against it."
)

# MediaPipe's image-to-tensor step, shared by every image model below.
WARP = (
    "ROI warp (MediaPipe ImageToTensor, OpenCV CPU path): take the rotated ROI rectangle in source pixels "
    "(center, width, height, rotation in radians, clockwise-positive in image coordinates since y points down); "
    "corners = cv::boxPoints order [bottom-left, top-left, top-right, bottom-right]; map them to tensor corners "
    "[(0,H),(0,0),(W,0),(W,H)] with a perspective (here affine) transform, i.e. corner-to-corner, not pixel-centre; "
    "sample the uint8 RGB source with bilinear interpolation (cv::warpPerspective, INTER_LINEAR) at integer tensor "
    "pixel coordinates; THEN normalise. Out-of-image pixels follow the border mode stated per model."
)
LETTERBOX = (
    "keep_aspect_ratio=true: before the warp, the ROI is enlarged (never cropped) to the tensor aspect ratio, "
    "padding equally on both sides (PadRoi); padding fractions are needed to map outputs back."
)

DETECTOR_DECODE = (
    "Decoding (TensorsToDetectionsCalculator): regressors[0,i,:] = [dx, dy, w, h, kp0x, kp0y, ... kp5x, kp5y] "
    "(reverse_output_order=true, so x before y). With anchor (ax, ay) in [0,1] and scale S = input size: "
    "cx = ax + dx/S, cy = ay + dy/S, w = w/S, h = h/S (fixed_anchor_size: anchor w=h=1); keypoint k = "
    "(ax + kx/S, ay + ky/S). Score = sigmoid(clamp(classificators[0,i,0], -100, 100)); keep score >= "
    "min_score_thresh. Then NMS: algorithm WEIGHTED, overlap IoU, min_suppression_threshold 0.3 (Tasks default "
    "FaceDetectorOptions.min_suppression_threshold). Coordinates are normalised to the letterboxed tensor; remove "
    "the letterbox padding to get image-normalised values. Keypoints (subject = the person): 0 right eye "
    "(image-left), 1 left eye (image-right), 2 nose tip, 3 mouth centre, 4 right ear tragion, 5 left ear tragion."
)

ANCHORS_RULE = (
    "Anchor generation (SsdAnchorsCalculator): for each layer l with stride s: feature map F = ceil(input/s); for "
    "y in 0..F, x in 0..F, emit anchors_per_cell anchors at center ((x+anchor_offset_x)/F, (y+anchor_offset_y)/F), "
    "w=h=1 (fixed_anchor_size). Consecutive layers with the same stride are merged into one feature map whose "
    "anchors_per_cell is the sum: with aspect_ratios=[1.0] and interpolated_scale_aspect_ratio=1.0 each layer "
    "contributes 2 anchors per cell, with interpolated_scale_aspect_ratio=0.0 it contributes 1. Order: layer group, "
    "then y, then x, then anchor index within the cell."
)


@dataclass(frozen=True)
class TensorSpec:
    name: str
    dtype: str
    shape: list
    layout: str | None = None
    color_order: str | None = None
    value_range: str | None = None
    description: str | None = None

    def to_json(self) -> dict:
        out = {"name": self.name, "dtype": self.dtype, "shape": self.shape}
        if self.layout:
            out["layout"] = self.layout
        if self.color_order:
            out["colorOrder"] = self.color_order
        if self.value_range:
            out["valueRange"] = self.value_range
        if self.description:
            out["description"] = self.description
        return out


@dataclass(frozen=True)
class ModelSpec:
    id: str
    role: str
    variant: str | None
    source_key: str
    member: str | None  # file inside a .task bundle
    rename: dict[str, str]  # upstream tensor name -> shipped name
    inputs: list[TensorSpec]
    outputs: list[TensorSpec]
    notes: str
    training_statement: str
    training_source: str
    commercial_use: str
    review_notes: str = ""
    converted: bool = True
    tolerances: dict[str, float] = field(default_factory=dict)  # per output, absolute

    @property
    def file(self) -> str:
        return f"{self.id}.onnx"


def _image_in(size: int, value_range: str, description: str) -> TensorSpec:
    return TensorSpec("input", "float32", [1, size, size, 3], "NHWC", "RGB", value_range, description)


MODELS: list[ModelSpec] = [
    ModelSpec(
        id="mp-face-detector-short",
        role="face-detector",
        variant="short-range",
        source_key="blaze_face_short_range",
        member=None,
        rename={},
        inputs=[_image_in(128, "(x-127.5)/127.5", "Whole image letterboxed to 128x128; padding = black.")],
        outputs=[
            TensorSpec("regressors", "float32", [1, 896, 16], description="Raw box + 6 keypoint offsets per anchor."),
            TensorSpec("classificators", "float32", [1, 896, 1], description="Raw score logit per anchor."),
        ],
        notes=" ".join(
            [
                "Short-range BlazeFace: faces within ~2 m, i.e. selfies. Identical bytes to the face_detector.tflite "
                "inside face_landmarker.task, so this is also the detector MediaPipe FaceLandmarker uses.",
                WARP,
                LETTERBOX,
                "Border mode: BORDER_ZERO (black, which becomes -1.0 after normalisation). ROI = whole image, "
                "rotation 0.",
                "Anchors: num_layers 4, strides [8,16,16,16], input 128x128, min_scale 0.1484375, max_scale 0.75, "
                "anchor_offset 0.5/0.5, aspect_ratios [1.0], interpolated_scale_aspect_ratio 1.0, "
                "fixed_anchor_size true -> 16x16x2 + 8x8x6 = 896 anchors.",
                ANCHORS_RULE,
                DETECTOR_DECODE,
                "Scale S = 128. MediaPipe min_score_thresh default 0.5.",
                "Landmark ROI from a detection (FaceDetectorGraph -> FaceLandmarker): rect = detection box in "
                "image pixels; rotation = 0 - atan2(-(kp1.y - kp0.y), kp1.x - kp0.x) using pixel coordinates "
                "(DetectionsToRects, start keypoint 0, end keypoint 1, target angle 0), normalised to [-pi, pi]; "
                "then scale width and height by 1.5 around the centre (RectTransformation, square_long false).",
            ]
        ),
        training_statement="Google in-house data. Model card (2021-06-09): 'trained and evaluated on consented images "
        'of people using a mobile AR application captured with smartphone cameras in various "in the wild" '
        "conditions'; 17 UN geographic subregions. No dataset size or public dataset named. Weights: 'LICENSED UNDER "
        "Apache License, Version 2.0'; no separate data licence or commercial restriction.",
        review_notes=SCOPE_CLAUSE + " Card operating range: faces within ~2 m, face box sides >= 20% of the image.",
        training_source=MP_CARDS + "MediaPipe%20BlazeFace%20Model%20Card%20(Short%20Range).pdf",
        commercial_use="allowed",
        tolerances={"regressors": 2e-3, "classificators": 1e-3},
    ),
    ModelSpec(
        id="mp-face-detector-full",
        role="face-detector",
        variant="full-range",
        source_key="face_detection_full_range",
        member=None,
        rename={"reshaped_regressor_face_4": "regressors", "reshaped_classifier_face_4": "classificators"},
        inputs=[_image_in(192, "(x-127.5)/127.5", "Whole image letterboxed to 192x192; padding = black.")],
        outputs=[
            TensorSpec("regressors", "float32", [1, 2304, 16], description="Raw box + 6 keypoint offsets per anchor."),
            TensorSpec("classificators", "float32", [1, 2304, 1], description="Raw score logit per anchor."),
        ],
        notes=" ".join(
            [
                "Full-range BlazeFace (dense variant): faces up to ~5 m; used for counting background faces and "
                "finding document portraits. Numerically identical to the Tasks test asset "
                "face_detection_full_range_with_metadata.tflite used for the goldens.",
                WARP,
                LETTERBOX,
                "Border mode: BORDER_ZERO (black -> -1.0). ROI = whole image, rotation 0.",
                "Anchors: num_layers 1, strides [4], input 192x192, min_scale 0.1484375, max_scale 0.75, "
                "anchor_offset 0.5/0.5, aspect_ratios [1.0], interpolated_scale_aspect_ratio 0.0, "
                "fixed_anchor_size true -> 48x48x1 = 2304 anchors.",
                ANCHORS_RULE,
                DETECTOR_DECODE,
                "Scale S = 192. MediaPipe min_score_thresh 0.6 in the legacy graph; Tasks uses the caller's "
                "min_detection_confidence (default 0.5).",
            ]
        ),
        training_statement="Google in-house data. Model card (2021-01-19): 'The dataset consists of 100+K images', "
        "'consented images of people using a mobile AR application'; 17 UN geographic subregions. Weights: "
        "'LICENSED UNDER Apache License, Version 2.0'; no separate data licence or commercial restriction.",
        review_notes=SCOPE_CLAUSE + " Card operating range: faces within ~5 m, face box sides >= 5% of the image.",
        training_source=MP_CARDS + "MediaPipe%20BlazeFace%20Model%20Card%20(Full%20Range).pdf",
        commercial_use="allowed",
        tolerances={"regressors": 2e-3, "classificators": 1e-3},
    ),
    ModelSpec(
        id="mp-face-landmarker",
        role="face-landmarker",
        variant="face-mesh-v2",
        source_key="face_landmarker_task",
        member="face_landmarks_detector.tflite",
        rename={
            "input_12": "input",
            "Identity": "landmarks",
            "Identity_1": "presence_logit",
            "Identity_2": "tongue_out",
        },
        inputs=[_image_in(256, "x/255  ([0,1])", "Rotated face ROI warped to 256x256 (no letterbox).")],
        outputs=[
            TensorSpec(
                "landmarks",
                "float32",
                [1, 1, 1, 1434],
                description="478 x (x, y, z), flattened. x, y in tensor pixels (0..256); z in the same scale "
                "as x, relative to the face centre (smaller = closer to the camera).",
            ),
            TensorSpec(
                "presence_logit",
                "float32",
                [1, 1, 1, 1],
                description="Face presence logit; presence = sigmoid(value), MediaPipe threshold "
                "min_face_presence_confidence (default 0.5).",
            ),
            TensorSpec(
                "tongue_out",
                "float32",
                [1, 1],
                description="Sigmoid score for 'tongue out of mouth'. Ignored by MediaPipe; do not rely on it.",
            ),
        ],
        notes=" ".join(
            [
                "MediaPipe Face Mesh v2 (face_landmarks_detector.tflite from face_landmarker.task).",
                WARP,
                "keep_aspect_ratio=false (ROI stretched to 256x256; MediaPipe ROIs are square here). "
                "Border mode: BORDER_REPLICATE.",
                "ROI from a detection: see mp-face-detector-short notes (box x1.5, rotation from keypoints 0->1). "
                "ROI from previous-frame landmarks (tracking; not used for still images): bounding box of all 478 "
                "landmarks, rotation from landmark 33 to 263 (target 0), scale 1.5, square_long true.",
                "Decoding (TensorsToLandmarks, normalize_z 1): landmark i = landmarks[3i..3i+3]; normalised "
                "x/256, y/256, z/256 within the ROI; then map through the ROI rect (rotate by roi.rotation around "
                "the ROI centre, scale by ROI width/height) to image-normalised coordinates; z is scaled by ROI "
                "width (LandmarkProjectionCalculator). Indices 468-472 = subject's right iris (468 centre, image-left "
                "eye), 473-477 = subject's left iris (473 centre, image-right eye).",
                "Alignment points for auraface-v1 (docs/architecture.md section 3.3): 468, 473, 1, 61, 291.",
            ]
        ),
        training_statement="Google in-house data. Model card (2022-09-15): images 'captured on a diverse set of "
        "smartphone cameras ... via an AR (Augmented Reality) application'; z from synthetic data. No dataset size; "
        "the card says nothing about consent. Weights: 'LICENSED UNDER Apache License, Version 2.0'.",
        review_notes="Card: 'Predicted face landmarks do not provide facial recognition or identification'; primary "
        "intended application 'AR entertainment'. No consent statement in the card.",
        training_source=MP_CARDS + "Model%20Card%20MediaPipe%20Face%20Mesh%20V2.pdf",
        commercial_use="allowed",
        tolerances={"landmarks": 1e-2, "presence_logit": 2e-3, "tongue_out": 1e-4},
    ),
    ModelSpec(
        id="mp-face-blendshapes",
        role="face-blendshapes",
        variant="blendshape-v2",
        source_key="face_landmarker_task",
        member="face_blendshapes.tflite",
        rename={"serving_default_input_points:0": "points", "StatefulPartitionedCall:0": "blendshapes"},
        inputs=[
            TensorSpec(
                "points",
                "float32",
                [1, 146, 2],
                description="(x, y) of the 146-landmark subset, in pixels of the FULL input image "
                "(image-normalised landmark x * image width, y * image height).",
            )
        ],
        outputs=[
            TensorSpec(
                "blendshapes",
                "float32",
                [52],
                description="52 coefficients in [0,1], ARKit order; index 0 '_neutral' is to be ignored.",
            )
        ],
        notes=" ".join(
            [
                "MediaPipe blendshape model (face_blendshapes.tflite from face_landmarker.task). No image input.",
                "Landmark subset (kLandmarksSubsetIdxs, in this order): 0,1,4,5,6,7,8,10,13,14,17,21,33,37,39,40,46,"
                "52,53,54,55,58,61,63,65,66,67,70,78,80,81,82,84,87,88,91,93,95,103,105,107,109,127,132,133,136,"
                "144,145,146,148,149,150,152,153,154,155,157,158,159,160,161,162,163,168,172,173,176,178,181,185,"
                "191,195,197,234,246,249,251,263,267,269,270,276,282,283,284,285,288,291,293,295,296,297,300,308,"
                "310,311,312,314,317,318,321,323,324,332,334,336,338,356,361,362,365,373,374,375,377,378,379,380,"
                "381,382,384,385,386,387,388,389,390,397,398,400,402,405,409,415,454,466,468,469,470,471,472,473,"
                "474,475,476,477.",
                "Output names (index order): _neutral, browDownLeft, browDownRight, browInnerUp, browOuterUpLeft, "
                "browOuterUpRight, cheekPuff, cheekSquintLeft, cheekSquintRight, eyeBlinkLeft, eyeBlinkRight, "
                "eyeLookDownLeft, eyeLookDownRight, eyeLookInLeft, eyeLookInRight, eyeLookOutLeft, eyeLookOutRight, "
                "eyeLookUpLeft, eyeLookUpRight, eyeSquintLeft, eyeSquintRight, eyeWideLeft, eyeWideRight, "
                "jawForward, jawLeft, jawOpen, jawRight, mouthClose, mouthDimpleLeft, mouthDimpleRight, "
                "mouthFrownLeft, mouthFrownRight, mouthFunnel, mouthLeft, mouthLowerDownLeft, mouthLowerDownRight, "
                "mouthPressLeft, mouthPressRight, mouthPucker, mouthRight, mouthRollLower, mouthRollUpper, "
                "mouthShrugLower, mouthShrugUpper, mouthSmileLeft, mouthSmileRight, mouthStretchLeft, "
                "mouthStretchRight, mouthUpperUpLeft, mouthUpperUpRight, noseSneerLeft, noseSneerRight. "
                "'Left'/'Right' refer to the subject (eyeBlinkLeft = subject's left eye = image-right).",
            ]
        ),
        training_statement="Google lab data. Model card (2022-11-11): 'All dataset samples were captured in a "
        "controlled environment in the lab' (multi-view captures fitted to GHUM meshes), 'trained on millions of "
        "samples generated with diverse identities'. Subject count and consent not stated. Weights: 'LICENSED UNDER "
        "Apache License, Version 2.0'.",
        review_notes="Card: blendshapes 'do not provide facial recognition or identification'; primary intended "
        "application 'AR entertainment'. No consent statement in the card.",
        training_source=MP_CARDS + "Model%20Card%20Blendshape%20V2.pdf",
        commercial_use="allowed",
        tolerances={"blendshapes": 1e-4},
    ),
    ModelSpec(
        id="mp-selfie-multiclass",
        role="segmenter",
        variant="selfie-multiclass-256",
        source_key="selfie_multiclass",
        member=None,
        rename={"input_29": "input", "Identity": "logits"},
        inputs=[_image_in(256, "(x-127.5)/127.5", "Head-and-shoulders ROI warped to 256x256 (no letterbox).")],
        outputs=[
            TensorSpec(
                "logits",
                "float32",
                [1, 256, 256, 6],
                "NHWC",
                description="Per-pixel class logits; apply softmax over the last axis. Classes: 0 background, "
                "1 hair, 2 body-skin, 3 face-skin, 4 clothes, 5 others (accessories).",
            )
        ],
        notes=" ".join(
            [
                "MediaPipe selfie_multiclass_256x256 (ImageSegmenter). Labels from the model's packed labels.txt; "
                "activation SOFTMAX from its SEGMENTER_METADATA. No custom ops (TRANSPOSE_CONV is a builtin), so no "
                "op lowering was needed.",
                WARP,
                "keep_aspect_ratio=false (ROI stretched to 256x256). Border mode: BORDER_REPLICATE. MediaPipe Tasks "
                "resizes the 256x256 softmax output back to the ROI/image size with bilinear interpolation for its "
                "confidence masks; category_mask = argmax.",
            ]
        ),
        training_statement="Google data; source, size and consent are not described. The model card (2023-05-10) "
        "has a single training sentence that appears copied from the Blendshape card ('various blendshape "
        "combinations'), so it is not relied on. Weights: 'LICENSED UNDER Apache License, Version 2.0'.",
        review_notes=SCOPE_CLAUSE + " Card fairness: mean IoU lowest for Monk tones 9+10 (68.25 vs 77.23 overall, "
        "256 model, n=31).",
        training_source=MP_CARDS + "Model%20Card%20Multiclass%20Segmentation.pdf",
        commercial_use="allowed",
        tolerances={"logits": 2e-3},
    ),
    ModelSpec(
        id="auraface-v1",
        role="face-embedder",
        variant="resnet100-arcface",
        source_key="auraface",
        member=None,
        rename={"data": "input", "1333": "embedding"},
        converted=False,
        inputs=[
            TensorSpec(
                "input",
                "float32",
                ["N", 3, 112, 112],
                "NCHW",
                "RGB",
                "(x-127.5)/127.5",
                description="Face aligned to the standard 112x112 ArcFace 5-point template "
                "(docs/architecture.md section 3.3).",
            )
        ],
        outputs=[
            TensorSpec(
                "embedding",
                "float32",
                ["N", 512],
                description="Un-normalised 512-d embedding; L2-normalise, then cosine similarity.",
            )
        ],
        notes=" ".join(
            [
                "fal/AuraFace-v1 recognition model ONLY (glintr100.onnx, PyTorch 2.3 export, opset 11). Shipped as "
                "the upstream graph with only its I/O tensors renamed (data -> input, 1333 -> embedding) and the "
                "batch dimension made symbolic; outputs are bit-identical to the upstream file.",
                "Normalisation and colour order follow InsightFace's ArcFaceONNX handling of graphs without a "
                "leading Sub/Mul (mean 127.5, std 127.5, swapRB from BGR, i.e. RGB input), as used by the AuraFace "
                "README.",
                "Not byte-identical to InsightFace antelopev2 glintr100.onnx (sha256 4ab1d643..., 260665334 bytes "
                "vs this 260694151 bytes). The other files in the AuraFace repository (scrfd_10g_bnkps, 2d106det, "
                "1k3d68, genderage) are InsightFace's non-commercial models; scrfd_10g_bnkps.onnx is byte-identical "
                "to antelopev2's. They are never fetched.",
            ]
        ),
        training_statement="fal.ai's model card states AuraFace 'was trained on a commercial dataset comprising "
        "face images from various sources' and 'on commercially and publicly available data sources to enable its "
        "usage in commercial setting'. The dataset is not named or published, so the training-data licence cannot "
        "be verified.",
        training_source="https://huggingface.co/fal/AuraFace-v1",
        commercial_use="unclear",
        review_notes="Training data undisclosed (ADR 0002): legal review required before general availability.",
        tolerances={"embedding": 0.0},
    ),
]

MODELS_BY_ID = {m.id: m for m in MODELS}
