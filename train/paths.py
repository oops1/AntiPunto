"""Где лежат данные: $ANTIPUNTO_DATA или data/ в корне репозитория."""

import os
from pathlib import Path

ROOT = Path(os.environ.get("ANTIPUNTO_DATA") or Path(__file__).resolve().parent.parent / "data")
CORPUS = ROOT / "corpus"
LAYOUT = ROOT / "layout"
