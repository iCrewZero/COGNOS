"""
LSTM Behavior Model for COGNOS/OS.
Trains on user workflow patterns, exports to ONNX for C++ runtime inference.
Target: <500k parameters, <2% CPU at inference time.

Privacy: all training data stays local. Model weights never uploaded.
Retrains weekly (not continuously) to prevent drift.
"""

from __future__ import annotations

import json
import logging
import os
import re
from dataclasses import dataclass
from datetime import datetime, UTC, timedelta
from pathlib import Path
from typing import Iterator

import numpy as np
import torch
import torch.nn as nn
from torch.utils.data import Dataset, DataLoader

log = logging.getLogger("cognos.lstm_model")

PREDICTOR_DIR = Path.home() / ".cognos" / "predictor"
AUDIT_LOG = Path.home() / ".cognos" / "audit.log"

# Feature dimensions
N_APP_CATEGORIES = 6    # coding, browser, terminal, media, office, other
N_DOMAIN_CATEGORIES = 6 # coding, writing, research, personal, gaming, other
N_DAY_OF_WEEK = 7
FEATURE_DIM = 2 + N_DAY_OF_WEEK + N_APP_CATEGORIES + N_DOMAIN_CATEGORIES + 2  # 23

APP_CATEGORIES = ["coding", "browser", "terminal", "media", "office", "other"]
DOMAIN_CATEGORIES = ["coding", "writing", "research", "personal", "gaming", "other"]
CODING_APPS = {"vscode", "code", "vim", "neovim", "nvim", "emacs", "jetbrains",
               "idea", "pycharm", "cursor", "zed"}
BROWSER_APPS = {"firefox", "chromium", "chrome", "brave", "safari"}
TERMINAL_APPS = {"foot", "alacritty", "kitty", "bash", "zsh", "fish", "wezterm"}
MEDIA_APPS = {"mpv", "vlc", "spotify", "rhythmbox", "kdenlive", "obs"}
OFFICE_APPS = {"libreoffice", "calc", "writer", "impress", "word", "excel"}

SEQUENCE_LENGTH = 20
HIDDEN_SIZE = 64
NUM_LAYERS = 2
DROPOUT = 0.3
BATCH_SIZE = 32
MAX_EPOCHS = 50
PATIENCE = 5
MC_PASSES = 10
CONFIDENCE_THRESHOLD = 0.85


# ─── Feature encoding ─────────────────────────────────────────────────────────

def encode_hour(hour: int) -> tuple[float, float]:
    """Cyclical encoding for hour of day."""
    angle = hour * 2 * np.pi / 24
    return float(np.sin(angle)), float(np.cos(angle))


def encode_day(day: int) -> list[float]:
    """One-hot encoding for day of week (0=Mon, 6=Sun)."""
    v = [0.0] * N_DAY_OF_WEEK
    v[day % N_DAY_OF_WEEK] = 1.0
    return v


def encode_app(app_name: str) -> list[float]:
    """One-hot encoding for app category."""
    name = app_name.lower()
    if any(a in name for a in CODING_APPS):
        idx = 0
    elif any(a in name for a in BROWSER_APPS):
        idx = 1
    elif any(a in name for a in TERMINAL_APPS):
        idx = 2
    elif any(a in name for a in MEDIA_APPS):
        idx = 3
    elif any(a in name for a in OFFICE_APPS):
        idx = 4
    else:
        idx = 5
    v = [0.0] * N_APP_CATEGORIES
    v[idx] = 1.0
    return v


def encode_domain(domain: str) -> list[float]:
    """One-hot encoding for domain."""
    idx = DOMAIN_CATEGORIES.index(domain) if domain in DOMAIN_CATEGORIES else 5
    v = [0.0] * N_DOMAIN_CATEGORIES
    v[idx] = 1.0
    return v


def encode_event(event: dict) -> list[float]:
    """Encode one event into a flat feature vector."""
    hour = int(event.get("time", "00:00").split(":")[0])
    day = int(event.get("day", 0)) % 7
    app = event.get("app", "other")
    domain = event.get("domain", "other")
    session_depth = min(float(event.get("session_depth", 0)) / 50.0, 1.0)
    minutes_since_last = float(event.get("minutes_since_last", 60))
    time_since_log = np.log1p(minutes_since_last) / np.log1p(1440)

    h_sin, h_cos = encode_hour(hour)
    features = (
        [h_sin, h_cos]
        + encode_day(day)
        + encode_app(app)
        + encode_domain(domain)
        + [session_depth, float(time_since_log)]
    )
    assert len(features) == FEATURE_DIM, f"Expected {FEATURE_DIM}, got {len(features)}"
    return features


# ─── Audit log parser ─────────────────────────────────────────────────────────

def parse_audit_log(audit_path: Path = AUDIT_LOG) -> list[list[dict]]:
    """
    Parse the audit log into training sequences.
    Each session becomes one sequence. Returns only sessions with ≥5 events.
    """
    if not audit_path.exists():
        return []

    sessions: dict[str, list[dict]] = {}

    for line in audit_path.read_text(errors="replace").splitlines():
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue

        session_id = entry.get("session") or entry.get("session_id", "unknown")
        if session_id == "unknown":
            continue

        ts_str = entry.get("ts", "")
        try:
            ts = datetime.fromisoformat(ts_str.replace("Z", "+00:00"))
        except ValueError:
            continue

        # Only include app-related events that tell us something about workflow
        action = entry.get("action", "")
        if action not in ("open_file", "open_app", "coding_task", "open_workspace"):
            continue

        event = {
            "time": ts.strftime("%H:%M"),
            "day": ts.weekday(),
            "app": _infer_app(entry),
            "domain": entry.get("domain", "other"),
            "session_depth": len(sessions.get(session_id, [])),
            "minutes_since_last": 60,  # simplified; proper version computes delta
        }

        if session_id not in sessions:
            sessions[session_id] = []
        sessions[session_id].append(event)

    return [events for events in sessions.values() if len(events) >= 5]


def _infer_app(entry: dict) -> str:
    """Infer the app category from an audit log entry."""
    target = entry.get("target", "").lower()
    note = entry.get("note", "").lower()
    for text in (target, note):
        for app_set, category in [
            (CODING_APPS, "vscode"),
            (BROWSER_APPS, "firefox"),
            (TERMINAL_APPS, "foot"),
            (MEDIA_APPS, "mpv"),
            (OFFICE_APPS, "libreoffice"),
        ]:
            if any(a in text for a in app_set):
                return category
    return "other"


# ─── Dataset ──────────────────────────────────────────────────────────────────

class WorkflowDataset(Dataset):
    """Sliding window over workflow event sequences."""

    def __init__(self, sequences: list[list[dict]], seq_len: int = SEQUENCE_LENGTH):
        self.samples: list[tuple[list, int, int]] = []  # (features, app_label, domain_label)

        for session in sequences:
            for i in range(len(session) - seq_len):
                window = session[i:i + seq_len]
                target_event = session[i + seq_len]
                features = [encode_event(e) for e in window]

                app_name = target_event.get("app", "other")
                domain = target_event.get("domain", "other")

                app_label = _app_to_label(app_name)
                domain_label = DOMAIN_CATEGORIES.index(domain) if domain in DOMAIN_CATEGORIES else 5

                self.samples.append((features, app_label, domain_label))

    def __len__(self) -> int:
        return len(self.samples)

    def __getitem__(self, idx: int):
        features, app_label, domain_label = self.samples[idx]
        x = torch.tensor(features, dtype=torch.float32)
        return x, torch.tensor(app_label), torch.tensor(domain_label)


def _app_to_label(app_name: str) -> int:
    """Convert app name string to category index."""
    name = app_name.lower()
    if any(a in name for a in CODING_APPS): return 0
    if any(a in name for a in BROWSER_APPS): return 1
    if any(a in name for a in TERMINAL_APPS): return 2
    if any(a in name for a in MEDIA_APPS): return 3
    if any(a in name for a in OFFICE_APPS): return 4
    return 5


# ─── Model ───────────────────────────────────────────────────────────────────

class WorkflowLSTM(nn.Module):
    """
    2-layer LSTM for workflow prediction.
    Small by design: <500k parameters, fast inference, works on CPU.
    """

    def __init__(
        self,
        input_size: int = FEATURE_DIM,
        hidden_size: int = HIDDEN_SIZE,
        num_layers: int = NUM_LAYERS,
        dropout: float = DROPOUT,
        n_app_classes: int = N_APP_CATEGORIES,
        n_domain_classes: int = N_DOMAIN_CATEGORIES,
    ):
        super().__init__()
        self.lstm = nn.LSTM(
            input_size=input_size,
            hidden_size=hidden_size,
            num_layers=num_layers,
            batch_first=True,
            dropout=dropout if num_layers > 1 else 0.0,
        )
        self.dropout = nn.Dropout(dropout)
        self.app_head = nn.Linear(hidden_size, n_app_classes)
        self.domain_head = nn.Linear(hidden_size, n_domain_classes)
        # Confidence: sigmoid scalar estimating the model's own certainty
        self.confidence_head = nn.Linear(hidden_size, 1)

    def forward(self, x: torch.Tensor):
        # x: (batch, seq_len, feature_dim)
        out, _ = self.lstm(x)
        last = self.dropout(out[:, -1, :])  # take last timestep
        app_logits = self.app_head(last)
        domain_logits = self.domain_head(last)
        confidence = torch.sigmoid(self.confidence_head(last))
        return app_logits, domain_logits, confidence


def count_parameters(model: nn.Module) -> int:
    return sum(p.numel() for p in model.parameters() if p.requires_grad)


# ─── Training ────────────────────────────────────────────────────────────────

def train(sequences: list[list[dict]] | None = None) -> WorkflowLSTM:
    """Train the LSTM on local audit log data."""
    PREDICTOR_DIR.mkdir(parents=True, exist_ok=True)

    if sequences is None:
        sequences = parse_audit_log()

    if len(sequences) < 10:
        log.warning("Insufficient training data (%d sessions) — using random init", len(sequences))
        model = WorkflowLSTM()
        torch.save(model.state_dict(), PREDICTOR_DIR / "model.pt")
        return model

    log.info("Training on %d sessions", len(sequences))

    # Split: hold out last 30% for validation (temporal split — recent data validates)
    split = int(len(sequences) * 0.7)
    train_seqs = sequences[:split]
    val_seqs = sequences[split:]

    train_dataset = WorkflowDataset(train_seqs)
    val_dataset = WorkflowDataset(val_seqs)

    if len(train_dataset) == 0:
        log.warning("No training samples after windowing")
        model = WorkflowLSTM()
        torch.save(model.state_dict(), PREDICTOR_DIR / "model.pt")
        return model

    train_loader = DataLoader(train_dataset, batch_size=BATCH_SIZE, shuffle=True)
    val_loader = DataLoader(val_dataset, batch_size=BATCH_SIZE)

    model = WorkflowLSTM()
    log.info("Model parameters: %d", count_parameters(model))
    assert count_parameters(model) < 500_000, "Model exceeds 500k parameter budget"

    optimizer = torch.optim.Adam(model.parameters(), lr=1e-3)
    app_criterion = nn.CrossEntropyLoss()
    domain_criterion = nn.CrossEntropyLoss()

    best_val_loss = float("inf")
    patience_count = 0
    checkpoint = PREDICTOR_DIR / "model.pt"

    for epoch in range(MAX_EPOCHS):
        # Training
        model.train()
        train_loss = 0.0
        for x, app_label, domain_label in train_loader:
            optimizer.zero_grad()
            app_logits, domain_logits, _ = model(x)
            loss = app_criterion(app_logits, app_label) + domain_criterion(domain_logits, domain_label)
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            optimizer.step()
            train_loss += loss.item()

        # Validation
        model.eval()
        val_loss = 0.0
        with torch.no_grad():
            for x, app_label, domain_label in val_loader:
                app_logits, domain_logits, _ = model(x)
                loss = app_criterion(app_logits, app_label) + domain_criterion(domain_logits, domain_label)
                val_loss += loss.item()

        val_loss /= max(len(val_loader), 1)
        log.info("Epoch %d/%d — val_loss=%.4f", epoch + 1, MAX_EPOCHS, val_loss)

        if val_loss < best_val_loss:
            best_val_loss = val_loss
            patience_count = 0
            torch.save(model.state_dict(), checkpoint)
        else:
            patience_count += 1
            if patience_count >= PATIENCE:
                log.info("Early stopping at epoch %d", epoch + 1)
                break

    # Reload best checkpoint
    model.load_state_dict(torch.load(checkpoint, weights_only=True))
    return model


# ─── ONNX export ─────────────────────────────────────────────────────────────

def export_onnx(model_path: str, output_path: str) -> None:
    """
    Export trained model to ONNX format for C++ runtime.
    Verifies exported model matches PyTorch output within 1e-4 tolerance.
    """
    import onnxruntime as ort

    # Load model
    model = WorkflowLSTM()
    model.load_state_dict(torch.load(model_path, weights_only=True))
    model.eval()

    # Enable dropout for MC uncertainty at inference time
    for m in model.modules():
        if isinstance(m, nn.Dropout):
            m.train()  # keep dropout active in ONNX export

    dummy_input = torch.randn(1, SEQUENCE_LENGTH, FEATURE_DIM)

    # Export
    torch.onnx.export(
        model,
        dummy_input,
        output_path,
        input_names=["sequence"],
        output_names=["app_logits", "domain_logits", "confidence"],
        dynamic_axes={
            "sequence": {0: "batch_size", 1: "seq_len"},
            "app_logits": {0: "batch_size"},
            "domain_logits": {0: "batch_size"},
            "confidence": {0: "batch_size"},
        },
        opset_version=17,
        training=torch.onnx.TrainingMode.TRAINING,  # keeps dropout active
    )

    # Verify: compare PyTorch vs ONNX output
    model.eval()  # pure eval for comparison
    with torch.no_grad():
        pt_app, pt_domain, pt_conf = model(dummy_input)

    session = ort.InferenceSession(output_path)
    ort_inputs = {"sequence": dummy_input.numpy()}
    ort_app, ort_domain, ort_conf = session.run(None, ort_inputs)

    tol = 1e-4
    assert np.allclose(pt_app.numpy(), ort_app, atol=tol), "app_logits mismatch"
    assert np.allclose(pt_domain.numpy(), ort_domain, atol=tol), "domain_logits mismatch"
    log.info("ONNX export verified — outputs match within %.0e", tol)

    # Write metadata
    meta = {
        "training_date": datetime.now(UTC).isoformat(),
        "n_parameters": count_parameters(model),
        "feature_dim": FEATURE_DIM,
        "sequence_length": SEQUENCE_LENGTH,
        "opset_version": 17,
    }
    meta_path = Path(output_path).with_suffix(".json")
    meta_path.write_text(json.dumps(meta, indent=2))
    log.info("Exported to %s with metadata", output_path)


def clear_training_data() -> None:
    """Delete all collected training sequences. Privacy guarantee."""
    audit = AUDIT_LOG
    if audit.exists():
        audit.unlink()
    log.info("Training data cleared")


# ─── Entry point ─────────────────────────────────────────────────────────────

if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    PREDICTOR_DIR.mkdir(parents=True, exist_ok=True)

    log.info("Parsing audit log...")
    sequences = parse_audit_log()
    log.info("Found %d sessions", len(sequences))

    log.info("Training model...")
    model = train(sequences)

    log.info("Exporting to ONNX...")
    pt_path = str(PREDICTOR_DIR / "model.pt")
    onnx_path = str(PREDICTOR_DIR / "model.onnx")
    export_onnx(pt_path, onnx_path)

    log.info("Done. Model at %s", onnx_path)
