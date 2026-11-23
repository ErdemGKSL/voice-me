"""Detached generation worker: `python3 higgs_worker.py <drive job dir>`.

Runs outside the notebook kernel (started with start_new_session), so a
disconnected browser or an interrupted cell does not stop it. Every finished
line is on Drive before the next one starts; `events.jsonl` records each
attempt and `manifest.json` is snapshotted every 20 lines or 60 seconds.
Touch /content/higgs/STOP to stop after the current line.
"""

import json
import os
import socket
import sys
import time
import traceback

sys.path.insert(0, "/content/higgs")
import higgs_common as hc  # noqa: E402

JOB = sys.argv[1]
LOCAL = hc.LOCAL
TMP = os.path.join(LOCAL, "tmp")
STOP = os.path.join(LOCAL, "STOP")
STATUS = os.path.join(LOCAL, "status.json")
LOCK = os.path.join(JOB, "worker.lock")
os.makedirs(TMP, exist_ok=True)
os.makedirs(os.path.join(JOB, "wavs"), exist_ok=True)
os.makedirs(os.path.join(JOB, "logs"), exist_ok=True)

log_file = open(os.path.join(LOCAL, "worker.log"), "a", encoding="utf-8")


def say(message):
    line = f"{hc.now()} {message}"
    log_file.write(line + "\n")
    log_file.flush()


def status(manifest, **extra):
    data = {"updated": time.time(), "counts": manifest["counts"], "total": len(manifest["items"]), **extra}
    with open(STATUS + ".tmp", "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False)
    os.replace(STATUS + ".tmp", STATUS)


def heartbeat():
    try:
        hc.write_json_atomic(LOCK, {"pid": os.getpid(), "host": socket.gethostname(),
                                    "heartbeat": time.time(), "at": hc.now()})
    except OSError:
        pass


def check_frozen(manifest):
    """The frozen inputs on Drive must still be the ones the job started with."""
    ref = os.path.join(JOB, "reference.wav")
    if hc.sha256_file(ref) != manifest["reference"]["sha256"]:
        raise SystemExit("reference.wav değişmiş; işi durduruyorum")
    if hc.sha256_file(os.path.join(JOB, "reading_list.txt")) != manifest["reading_list"]["sha256"]:
        raise SystemExit("reading_list.txt değişmiş; işi durduruyorum")
    if hc.sha256_text(json.dumps(manifest["config"], sort_keys=True)) != manifest["config_sha256"]:
        raise SystemExit("config değişmiş; işi durduruyorum")


def main():
    manifest = hc.load_manifest(JOB)
    check_frozen(manifest)
    report = hc.reconcile(JOB, manifest)
    if report["reset"] or report["adopted"]:
        say(f"uzlaştırma: {len(report['reset'])} satır yeniden üretilecek, {len(report['adopted'])} dosya benimsendi")
    hc.save_manifest(JOB, manifest, state="running")
    config = manifest["config"]
    ref_text = manifest["reference"]["text"]
    # The server reads the reference from local disk, never through Drive's FUSE.
    ref_local = os.path.join(LOCAL, "reference.wav")
    hc.copy_atomic(os.path.join(JOB, "reference.wav"), ref_local)

    hc.start_server(log_dir=os.path.join(JOB, "logs"), say=say)
    max_attempts = config["max_attempts"]
    last_snapshot, since_snapshot = time.time(), 0
    consecutive_failures, restarts_in_a_row = 0, 0
    started, done_this_run = time.time(), 0
    heartbeat()

    for index, item in enumerate(manifest["items"]):
        if item["status"] != "pending":
            continue
        if os.path.exists(STOP):
            say("STOP dosyası bulundu; duruyorum")
            break
        error = None
        while item["status"] == "pending" and item["attempts"] < max_attempts:
            attempt = item["attempts"]
            seed = config["base_seed"] + index + 1000 * attempt
            item["attempts"] += 1
            status(manifest, current=item["id"], text=item["text"][:120], attempt=item["attempts"],
                   rate=(time.time() - started) / max(1, done_this_run), done_this_run=done_this_run)
            tmp = os.path.join(TMP, item["id"] + ".wav")
            t0 = time.time()
            try:
                wav = hc.synth(item["text"], config, seed, ref_local, ref_text, timeout=hc.read_timeout(item["text"]))
                with open(tmp, "wb") as f:
                    f.write(wav)
                error = hc.validate(tmp, item["text"], config)
                if error:
                    raise ValueError(error)
                duration = hc.wav_duration(tmp)
                hc.copy_atomic(tmp, os.path.join(JOB, item["file"]))
                gen_s = round(time.time() - t0, 2)
                item.update(status="done", seed=seed, bytes=len(wav), duration_s=round(duration, 2),
                            gen_s=gen_s, error=None, finished_at=hc.now())
                hc.append_event(JOB, {"id": item["id"], "attempt": item["attempts"], "seed": seed, "status": "done",
                                      "bytes": len(wav), "duration_s": item["duration_s"], "gen_s": gen_s})
                consecutive_failures, restarts_in_a_row = 0, 0
                done_this_run += 1
                status(manifest, last_done=item["id"], last_file=tmp, done_this_run=done_this_run,
                       rate=(time.time() - started) / max(1, done_this_run))
            except hc.RequestRejected as rejected:  # a settings problem, the same for every line
                # Not the line's fault: no event, so the next run keeps its seed.
                item["attempts"] -= 1
                hc.save_manifest(JOB, manifest, state="halted")
                raise SystemExit(f"sunucu isteği reddetti, ayarları kontrol et: {rejected}")
            except ValueError as invalid:  # the clip came back but looks wrong: new seed
                error = str(invalid)
                say(f"{item['id']} deneme {item['attempts']}: {error}")
                hc.append_event(JOB, {"id": item["id"], "attempt": item["attempts"], "seed": seed,
                                      "status": "attempt_failed", "error": error})
            except Exception as failure:  # noqa: BLE001 - server/network trouble
                error = f"{type(failure).__name__}: {failure}"
                say(f"{item['id']} deneme {item['attempts']}: {error}")
                hc.append_event(JOB, {"id": item["id"], "attempt": item["attempts"], "seed": seed,
                                      "status": "attempt_failed", "error": error[:500]})
                if not hc.healthy():
                    restarts_in_a_row += 1
                    if restarts_in_a_row > 3:
                        hc.save_manifest(JOB, manifest, state="halted")
                        raise SystemExit("sunucu üst üste 3 kez yeniden başlatılamadı")
                    say("sunucu yanıt vermiyor; yeniden başlatıyorum")
                    time.sleep(5 * 3 ** (restarts_in_a_row - 1))
                    try:
                        hc.start_server(log_dir=os.path.join(JOB, "logs"), say=say)
                    except RuntimeError as start_error:
                        say(str(start_error)[-500:])
            finally:
                if os.path.exists(tmp) and item["status"] != "done":
                    os.remove(tmp)
        if item["status"] == "pending":
            item.update(status="failed", error=error, finished_at=hc.now())
            hc.append_event(JOB, {"id": item["id"], "attempt": item["attempts"], "status": "failed", "error": error})
            consecutive_failures += 1
            if consecutive_failures >= 10:
                hc.save_manifest(JOB, manifest, state="halted")
                raise SystemExit("üst üste 10 satır başarısız; ayarları kontrol et")
        since_snapshot += 1
        if since_snapshot >= 20 or time.time() - last_snapshot > 60:
            hc.save_manifest(JOB, manifest)
            hc.write_metadata(JOB, manifest)
            heartbeat()
            since_snapshot, last_snapshot = 0, time.time()

    counts = hc.count(manifest)
    hc.save_manifest(JOB, manifest, state="complete" if counts["pending"] == 0 else "halted")
    hc.write_metadata(JOB, manifest)
    status(manifest, finished=True, done_this_run=done_this_run)
    say(f"bitti: {counts}")


if __name__ == "__main__":
    try:
        main()
    except SystemExit as stop:
        say(f"durdu: {stop}")
        try:
            with open(STATUS, encoding="utf-8") as f:
                data = json.load(f)
        except (OSError, ValueError):
            data = {}
        data.update(finished=True, halted=str(stop))
        with open(STATUS, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False)
    except Exception:  # noqa: BLE001
        say("çöktü:\n" + traceback.format_exc())
        raise
    finally:
        try:
            import shutil
            shutil.copyfile(os.path.join(LOCAL, "worker.log"), os.path.join(JOB, "logs", "worker-latest.log"))
            os.remove(LOCK)
        except OSError:
            pass
