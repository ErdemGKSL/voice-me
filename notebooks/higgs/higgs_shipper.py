"""Back up finished lines to a Discord webhook in batches, as they finish.

    python3 higgs_shipper.py <job dir> [batch_size]

The webhook URL is read from /content/higgs/webhook.txt (kept off Drive and
out of the repository). Runs next to the worker and never touches its files
except reading them. Every `batch_size` finished lines become one batch:
the clips as FLAC (lossless, about half the size of WAV) plus that batch's
`metadata.csv` rows, zipped into parts under Discord's upload limit, then a
small `state` part with the current manifest and journal. Each batch is
also kept in /content/ship/ for pulling elsewhere.

What has been sent is recorded in `<job>/shipped.json` after every part, so
a restart resumes mid-batch without sending a part twice. When the worker
has stopped and fewer than `batch_size` lines remain unsent, they go as a
final short batch.
"""

import gzip
import io
import json
import os
import subprocess
import sys
import time
import zipfile

import requests

sys.path.insert(0, "/content/higgs")
import higgs_common as hc  # noqa: E402

JOB = sys.argv[1]
BATCH = int(sys.argv[2]) if len(sys.argv) > 2 else 250
WEBHOOK = open("/content/higgs/webhook.txt").read().strip()
SHIP = "/content/ship"
STATE = os.path.join(JOB, "shipped.json")
PART_LIMIT = 9_000_000  # Discord's free upload limit is 10 MB per message; leave room for the form
os.makedirs(SHIP, exist_ok=True)
log = open("/content/higgs/shipper.log", "a", encoding="utf-8")


def say(message):
    log.write(f"{hc.now()} {message}\n")
    log.flush()


def load_state():
    try:
        return hc.read_json(STATE)
    except (OSError, ValueError):
        return {"batches": [], "shipped_ids": []}


def post(filename, payload, content):
    """Send one file; waits out rate limits, retries transient failures."""
    for attempt in range(8):
        try:
            response = requests.post(WEBHOOK + "?wait=true", data={"content": content},
                                     files={"file": (filename, payload)}, timeout=120)
        except requests.RequestException as error:
            say(f"gönderim hatası ({filename}): {error}")
            time.sleep(min(300, 5 * 2 ** attempt))
            continue
        if response.status_code == 429:
            time.sleep(float(response.json().get("retry_after", 5)) + 0.5)
            continue
        if response.ok:
            return
        say(f"Discord {response.status_code} ({filename}): {response.text[:200]}")
        if 400 <= response.status_code < 500:
            raise SystemExit(f"Discord isteği reddetti: {response.status_code} {response.text[:200]}")
        time.sleep(min(300, 5 * 2 ** attempt))
    raise RuntimeError(f"{filename} gönderilemedi")


def flac(path):
    return subprocess.run(["ffmpeg", "-loglevel", "error", "-i", path, "-c:a", "flac",
                           "-compression_level", "8", "-f", "flac", "-"],
                          check=True, capture_output=True).stdout


def build_parts(number, items):
    """Zip the batch's clips into parts under PART_LIMIT; each part carries
    the metadata rows of its own clips."""
    parts, current, size = [], [], 0
    for item in items:
        data = flac(os.path.join(JOB, item["file"]))
        if current and size + len(data) > PART_LIMIT:
            parts.append(current)
            current, size = [], 0
        current.append((item, data))
        size += len(data)
    if current:
        parts.append(current)
    zips = []
    for part_no, part in enumerate(parts, 1):
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w", zipfile.ZIP_STORED) as z:  # FLAC is already compressed
            rows = [f"{item['id']}|{hc.strip_tags(item['text'])}" for item, _ in part]
            z.writestr("metadata.csv", "\n".join(rows) + "\n")
            for item, data in part:
                z.writestr(f"wavs/{item['id']}.flac", data)
        zips.append((f"batch{number:04d}_part{part_no:02d}of{len(parts):02d}.zip", buffer.getvalue(),
                     part[0][0]["id"], part[-1][0]["id"], len(part)))
    return zips


def state_part(number):
    """The job's manifest and journal, gzipped: enough to rebuild or resume."""
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as z:
        for name in ("manifest.json", "events.jsonl", "config.json", "reference.txt", "reading_list.txt"):
            path = os.path.join(JOB, name)
            if os.path.exists(path):
                z.write(path, name)
        z.write(os.path.join(JOB, "reference.wav"), "reference.wav")
    return f"batch{number:04d}_state.zip", buffer.getvalue()


def worker_running():
    return subprocess.run("pgrep -f '[h]iggs_worker.py'", shell=True, capture_output=True).returncode == 0


def main():
    state = load_state()
    shipped = set(state["shipped_ids"])
    say(f"başladı: {len(shipped)} satır daha önce gönderilmiş")
    while True:
        manifest = hc.load_manifest(JOB)
        ready = [i for i in manifest["items"] if i["status"] == "done" and i["id"] not in shipped
                 and hc.is_valid_wav(os.path.join(JOB, i["file"]), i.get("bytes"))]
        ready.sort(key=lambda i: i["id"])
        running = worker_running()
        if len(ready) < BATCH and (running or not ready):
            if not running and not ready:
                say("işçi durdu ve gönderilecek satır kalmadı; çıkıyorum")
                return
            time.sleep(60)
            continue
        batch = ready[:BATCH]
        number = len(state["batches"]) + 1
        record = next((b for b in state["batches"] if b.get("open")), None)
        if record:  # a batch cut off by a crash: finish exactly those lines
            number = record["number"]
            by_id = {i["id"]: i for i in manifest["items"]}
            batch = [by_id[i] for i in record["ids"]]
        else:
            record = {"number": number, "ids": [i["id"] for i in batch], "sent_parts": [], "open": True}
            state["batches"].append(record)
            hc.write_json_atomic(STATE, state)
        parts = build_parts(number, batch)
        local = os.path.join(SHIP, f"batch{number:04d}.zip")
        with zipfile.ZipFile(local + ".tmp", "w", zipfile.ZIP_STORED) as z:
            for name, payload, *_ in parts:
                z.writestr(name, payload)
        os.replace(local + ".tmp", local)
        for name, payload, first, last, count in parts:
            if name in record["sent_parts"]:
                continue
            post(name, payload, f"Higgs veri seti · paket {number} · {name} · {first}–{last} ({count} satır)")
            record["sent_parts"].append(name)
            hc.write_json_atomic(STATE, state)
        name, payload = state_part(number)
        post(name, payload, f"Higgs veri seti · paket {number} · durum (manifest, günlük, referans) · "
                            f"toplam {manifest['counts']['done']}/{len(manifest['items'])} bitti")
        record["open"] = False
        state["shipped_ids"].extend(record["ids"])
        shipped.update(record["ids"])
        hc.write_json_atomic(STATE, state)
        say(f"paket {number} gönderildi: {record['ids'][0]}–{record['ids'][-1]}, {len(parts)} parça")


if __name__ == "__main__":
    main()
