"""Shared helpers for the Higgs TTS dataset notebook and its worker.

Written to /content/higgs/higgs_common.py by the notebook; both the notebook
cells and the detached worker (`higgs_worker.py`) import it, so the server
command, the request shape and the validation rules live in one place.

Truth lives on Drive in three layers: the WAV files, an append-only
`events.jsonl`, and a `manifest.json` snapshot. `load_manifest` replays the
journal over the snapshot and `reconcile` checks it against the files, so a
stale snapshot after a crash costs at most a few re-generated lines.
"""

import datetime
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import time

import requests

LOCAL = "/content/higgs"
SERVER_URL = "http://localhost:8000"
MODEL = "bosonai/higgs-tts-3-4b"
SGLANG_OMNI = "0.2.0"
SAMPLE_RATE = 24000
FRAMES_PER_SECOND = 25
SCHEMA_VERSION = 1

# Voice presets. Without a reference clip the model picks a voice from the
# seed, so a preset is "this seed + these tags speaking PRESET_TEXT"; the
# notebook generates that clip once and then clones it for every line, which
# keeps one voice across the whole reading list. F0 is the median pitch
# measured on PRESET_TEXT (librosa.pyin) on an L4 — a label, not a promise.
PRESET_TEXT = (
    "Merhaba, ben Voice Me için hazırlanmış bir deneme sesiyim. Bugün size bu "
    "modelin Türkçeyi ne kadar doğal konuşabildiğini göstereceğim."
)
#
# Findings behind them (L4, sglang-omni 0.2.0): with no tag every seed tried
# came out female (171-210 Hz); `<|prosody:pitch_low|>` gives male voices but
# deep ones (72-112 Hz); the `instructions` field has no effect. A normal male
# voice comes from re-voicing a pitch_low voice with `<|prosody:pitch_high|>`
# ("restage", ~126 Hz; lines cloned from it land ~115 Hz). Shifting a voice up
# with DSP worked on paper but sounded crackly, so no preset uses it. Same seed + same
# text = same clip.
VOICE_PRESETS = {
    "Erkek – normal, seed 38 (~115–126 Hz)": {
        "seed": 38, "tags": "<|prosody:pitch_low|>",
        # This exact text and seed gave the ~126 Hz clip that was picked by
        # ear; other texts move the pitch (the preset text gave ~141 Hz).
        "restage": {"seed": 42, "tags": "<|prosody:pitch_high|>", "text": (
            "Sabah kahvemi içip pencereden dışarı baktım; yağmur yeni dinmişti ve "
            "sokaklar ışıl ışıldı. Bugün güzel bir gün olacak gibi."
        )},
    },
    "Erkek – tok, seed 39 (~112 Hz)": {"seed": 39, "tags": "<|prosody:pitch_low|>"},
    "Erkek – kalın (~80 Hz)": {"seed": 5, "tags": "<|prosody:pitch_low|>"},
    "Kadın – orta (~190 Hz)": {"seed": 10, "tags": ""},
    "Kadın – ince (~210 Hz)": {"seed": 13, "tags": ""},
}


def make_preset_reference(name, out_path):
    """Generate a preset's reference clip at out_path; returns its transcript."""
    preset = VOICE_PRESETS[name]
    params = {"temperature": 0.8, "top_k": 50, "max_new_tokens": 1024}
    with open(out_path, "wb") as f:
        f.write(synth(preset["tags"] + PRESET_TEXT, params, seed=preset["seed"]))
    if "restage" in preset:
        stage1 = out_path + ".stage1.wav"
        os.replace(out_path, stage1)
        restage = preset["restage"]
        text = restage.get("text", PRESET_TEXT)
        with open(out_path, "wb") as f:
            f.write(synth(restage["tags"] + text, params, seed=restage["seed"],
                          ref_path=stage1, ref_text=PRESET_TEXT))
        return text
    return PRESET_TEXT

# The model's 43 control tags (PROMPTING.md in the model repo). Anything else
# inside <|...|> is read aloud or degrades the output.
KNOWN_TAGS = {
    "emotion": {
        "affection", "amusement", "anger", "arousal", "awe", "bitterness",
        "confusion", "contemplation", "contentment", "determination",
        "disgust", "elation", "enthusiasm", "fear", "helplessness", "longing",
        "pride", "relief", "sadness", "shame", "surprise",
    },
    "prosody": {
        "speed_very_slow", "speed_slow", "speed_fast", "speed_very_fast",
        "pitch_low", "pitch_high", "expressive_high", "expressive_low",
        "pause", "long_pause",
    },
    "style": {"singing", "shouting", "whispering"},
    "sfx": {
        "cough", "laughter", "crying", "screaming", "burping", "humming",
        "sigh", "sniff", "sneeze",
    },
}
TAG_RE = re.compile(r"<\|([a-z]+):([a-z_]+)\|>")


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def sha256_text(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def unknown_tags(text):
    """Tags in `text` the model does not know, as "category:tag" strings."""
    bad = [f"{c}:{t}" for c, t in TAG_RE.findall(text) if t not in KNOWN_TAGS.get(c, ())]
    # A stray "<|" that is not a well-formed tag is also a problem.
    if text.count("<|") != len(TAG_RE.findall(text)):
        bad.append("malformed <|...|>")
    return bad


def strip_tags(text):
    """The text as spoken, for metadata.csv: tags removed, spaces collapsed."""
    return re.sub(r"\s+", " ", TAG_RE.sub("", text)).strip()


# ---- Drive I/O: retried, and atomic where it matters ----------------------

def retry_io(fn, *args, tries=5, **kwargs):
    for attempt in range(tries):
        try:
            return fn(*args, **kwargs)
        except OSError:
            if attempt == tries - 1:
                raise
            time.sleep(2 ** attempt)


def _fsync(f):
    f.flush()
    try:
        os.fsync(f.fileno())
    except OSError:
        pass  # best-effort on Drive's FUSE mount


def _write_json_atomic(path, obj):
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(obj, f, ensure_ascii=False, indent=1)
        _fsync(f)
    os.replace(tmp, path)


def write_json_atomic(path, obj):
    retry_io(_write_json_atomic, path, obj)


def _copy_atomic(src, dst):
    part = dst + ".part"
    shutil.copyfile(src, part)
    with open(part, "rb+") as f:
        _fsync(f)
    os.replace(part, dst)


def copy_atomic(src, dst):
    retry_io(_copy_atomic, src, dst)


def _append_line(path, line):
    with open(path, "a", encoding="utf-8") as f:
        f.write(line + "\n")
        _fsync(f)


def append_event(job_dir, event):
    event = {"ts": now(), **event}
    retry_io(_append_line, os.path.join(job_dir, "events.jsonl"), json.dumps(event, ensure_ascii=False))


def read_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


# ---- Manifest --------------------------------------------------------------

def count(manifest):
    counts = {"pending": 0, "done": 0, "failed": 0}
    for item in manifest["items"]:
        counts[item["status"]] += 1
    manifest["counts"] = counts
    return counts


def load_manifest(job_dir):
    """manifest.json with events.jsonl replayed over it."""
    manifest = read_json(os.path.join(job_dir, "manifest.json"))
    by_id = {item["id"]: item for item in manifest["items"]}
    events = os.path.join(job_dir, "events.jsonl")
    if os.path.exists(events):
        with open(events, encoding="utf-8") as f:
            for line in f:
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue  # a line cut off by a crash
                item = by_id.get(event.get("id"))
                if item is None:
                    continue
                item["attempts"] = max(item["attempts"], event.get("attempt", 0))
                if event["status"] == "done":
                    for key in ("seed", "bytes", "duration_s", "gen_s"):
                        item[key] = event.get(key)
                    item.update(status="done", error=None, finished_at=event["ts"])
                elif event["status"] == "failed":
                    item.update(status="failed", error=event.get("error"), finished_at=event["ts"])
                elif event["status"] == "reset":
                    item.update(status="pending", attempts=0, error=None)
                elif event["status"] == "attempt_failed" and item["status"] != "done":
                    item["error"] = event.get("error")
    count(manifest)
    return manifest


def is_valid_wav(path, expected_bytes=None):
    try:
        size = os.path.getsize(path)
        if expected_bytes and size != expected_bytes:
            return False
        with open(path, "rb") as f:
            return f.read(4) == b"RIFF" and size > 1000
    except OSError:
        return False


def reconcile(job_dir, manifest):
    """Make the manifest agree with the files on Drive. Returns a report."""
    reset, adopted = [], []
    for item in manifest["items"]:
        path = os.path.join(job_dir, item["file"])
        if item["status"] == "done" and not is_valid_wav(path, item.get("bytes")):
            # attempts=0 so the line is redone with the seed it had before.
            item.update(status="pending", attempts=0, error="file missing or incomplete on Drive")
            reset.append(item["id"])
        elif item["status"] != "done" and is_valid_wav(path):
            item.update(status="done", bytes=os.path.getsize(path), error=None)
            adopted.append(item["id"])
    count(manifest)
    return {"reset": reset, "adopted": adopted}


def save_manifest(job_dir, manifest, state=None):
    if state:
        manifest["state"] = state
    manifest["updated_at"] = now()
    count(manifest)
    write_json_atomic(os.path.join(job_dir, "manifest.json"), manifest)


def write_metadata(job_dir, manifest):
    """LJSpeech/Piper style `id|text` for the finished lines."""
    rows = [f"{item['id']}|{strip_tags(item['text'])}" for item in manifest["items"] if item["status"] == "done"]
    path = os.path.join(job_dir, "metadata.csv")

    def write():
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            f.write("\n".join(rows) + ("\n" if rows else ""))
            _fsync(f)
        os.replace(tmp, path)

    retry_io(write)


# ---- Server ----------------------------------------------------------------

SERVER_SCRIPT = f"""#!/bin/bash
# TORCHDYNAMO_DISABLE: without it startup spends 30+ minutes in torch.compile.
# cuda_graph_max_bs must cover max_running_requests or the server refuses.
export TORCHDYNAMO_DISABLE=1
exec sgl-omni serve --model-path {MODEL} --port 8000 \\
  --tts_engine.factory.cuda_graph_max_bs 1 \\
  --tts_engine.factory.max_running_requests 1
"""


def healthy(timeout=3):
    try:
        return requests.get(SERVER_URL + "/health", timeout=timeout).ok
    except requests.RequestException:
        return False


def _server_pid():
    try:
        with open(os.path.join(LOCAL, "server.pid")) as f:
            return int(f.read())
    except (OSError, ValueError):
        return None


def _alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def stop_server():
    pid = _server_pid()
    if pid and _alive(pid):
        try:
            os.killpg(pid, signal.SIGTERM)
            for _ in range(20):
                if not _alive(pid):
                    break
                time.sleep(1)
            os.killpg(pid, signal.SIGKILL)
        except OSError:
            pass
    # Stage workers are separate processes; make sure the GPU is free.
    # "[s]" keeps the pattern from matching this shell's own command line.
    subprocess.run("pkill -9 -f '[s]gl-omni serve'; pkill -9 -f '[s]glang_omni'", shell=True)
    time.sleep(3)


def start_server(log_dir=None, wait_s=900, say=print):
    """Start the server (stopping any old one) and wait until it is healthy."""
    if healthy():
        return True
    stop_server()
    os.makedirs(LOCAL, exist_ok=True)
    script = os.path.join(LOCAL, "server.sh")
    with open(script, "w") as f:
        f.write(SERVER_SCRIPT)
    log_path = os.path.join(LOCAL, "server.log")
    log = open(log_path, "ab")
    proc = subprocess.Popen(["bash", script], stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    with open(os.path.join(LOCAL, "server.pid"), "w") as f:
        f.write(str(proc.pid))
    started = time.time()
    while time.time() - started < wait_s:
        if healthy():
            say(f"sunucu hazır ({time.time() - started:.0f} sn)")
            if log_dir:
                try:
                    shutil.copyfile(log_path, os.path.join(log_dir, "server-latest.log"))
                except OSError:
                    pass
            return True
        if proc.poll() is not None:
            break
        time.sleep(5)
    with open(log_path, errors="replace") as f:
        tail = f.read()[-3000:]
    raise RuntimeError("sunucu başlamadı. server.log sonu:\n" + tail)


# ---- Synthesis and validation ------------------------------------------------

def synth(text, config, seed, ref_path=None, ref_text=None, timeout=600):
    """POST one line; returns WAV bytes or raises RuntimeError."""
    body = {
        "input": config.get("prefix_tags", "") + text,
        "temperature": config["temperature"],
        "top_k": config["top_k"],
        "max_new_tokens": config["max_new_tokens"],
        "seed": seed,
    }
    if config.get("top_p"):
        body["top_p"] = config["top_p"]
    if config.get("repetition_penalty"):
        body["repetition_penalty"] = config["repetition_penalty"]
    if config.get("language") and config["language"] != "auto":
        body["language"] = config["language"]
    if ref_path:
        body["references"] = [{"audio_path": ref_path, "text": ref_text}]
    response = requests.post(SERVER_URL + "/v1/audio/speech", json=body, timeout=(10, timeout))
    if not response.ok:
        raise RuntimeError(f"HTTP {response.status_code}: {response.text[:300]}")
    if response.content[:4] != b"RIFF":
        raise RuntimeError("yanıt WAV değil: " + response.content[:200].decode("utf-8", "replace"))
    return response.content


def read_timeout(text):
    """Seconds to wait for one line: generous, about real time on an L4."""
    return max(120, 8 * len(strip_tags(text)) / 14)


def validate(path, text, config):
    """None if the clip looks right, else the reason. Catches silence,
    truncation and wild length; it cannot catch wrong words."""
    import numpy as np
    import soundfile as sf

    try:
        audio, rate = sf.read(path, dtype="float32", always_2d=True)
    except Exception as error:  # noqa: BLE001 - any decode failure is a bad clip
        return f"okunamadı: {error}"
    audio = audio.mean(axis=1)
    duration = len(audio) / rate
    chars = max(1, len(strip_tags(text)))
    cap = config["max_new_tokens"] / FRAMES_PER_SECOND
    if duration < 0.4:
        return f"çok kısa ({duration:.1f} sn)"
    if duration >= 0.97 * cap:
        return f"kesilmiş görünüyor ({duration:.1f} sn, sınır {cap:.0f} sn)"
    if duration < chars / 28:
        return f"metne göre çok kısa ({duration:.1f} sn, {chars} karakter)"
    if duration > chars / 5 + 1.5:
        return f"metne göre çok uzun ({duration:.1f} sn, {chars} karakter)"
    frame = int(0.02 * rate)
    frames = audio[: len(audio) // frame * frame].reshape(-1, frame)
    rms = np.sqrt((frames ** 2).mean(axis=1) + 1e-12)
    if 20 * np.log10(rms.mean() + 1e-12) < -45:
        return "neredeyse sessiz"
    silent = rms < 10 ** (-45 / 20)
    if silent.mean() > 0.5:
        return f"%{silent.mean() * 100:.0f} sessizlik"
    trailing = 0
    for is_silent in silent[::-1]:
        if not is_silent:
            break
        trailing += 1
    if trailing * 0.02 > 2.0:
        return f"sonda {trailing * 0.02:.1f} sn sessizlik"
    return None


def wav_duration(path):
    import soundfile as sf

    info = sf.info(path)
    return info.frames / info.samplerate
