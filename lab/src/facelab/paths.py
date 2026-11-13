"""Repository-relative locations used by every lab command."""

from pathlib import Path

LAB_DIR = Path(__file__).resolve().parents[2]
REPO_ROOT = LAB_DIR.parent

MODELS_DIR = REPO_ROOT / "models"
UPSTREAM_DIR = MODELS_DIR / "upstream"  # git-ignored download cache
MANIFEST_PATH = MODELS_DIR / "manifest.json"

CONTRACTS_DIR = REPO_ROOT / "contracts"
MANIFEST_SCHEMA_PATH = CONTRACTS_DIR / "model-manifest.schema.json"

TESTDATA_DIR = REPO_ROOT / "testdata"
IMAGES_DIR = TESTDATA_DIR / "images"
DOCUMENTS_DIR = TESTDATA_DIR / "documents"
GOLDEN_DIR = TESTDATA_DIR / "golden"

WORK_DIR = LAB_DIR / "work"  # git-ignored scratch (reports, intermediates)
