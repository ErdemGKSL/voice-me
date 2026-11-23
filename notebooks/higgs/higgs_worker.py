"""Detached generation worker: `python3 higgs_worker.py <drive job dir> [concurrency]`.

Runs outside the notebook kernel (started with start_new_session), so a
disconnected browser or an interrupted cell does not stop it. `concurrency`
lines are generated at once (the GPU batches them); each one is on Drive
before it counts as done, `events.jsonl` records every attempt and
`manifest.json` is snapshotted every 60 seconds. Touch /content/higgs/STOP to
stop once the lines in flight finish.
"""

import concurrent.futures
import json
import os
import socket
import sys
import threading
import time
import traceback

sys.path.insert(0, "/content/higgs")
import higgs_common as hc  # noqa: E402

JOB = sys.argv[1]
# Lines generated at once; the server is started with the same capacity.
CONCURRENCY = int(sys.argv[2]) if len(sys.argv) > 2 else 1
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


class Run:
    """State shared by the generation threads; every change holds `lock`."""

    def __init__(self, manifest, config):
        self.manifest = manifest
        self.config = config
        self.lock = threading.Lock()
        self.server_lock = threading.Lock()
        self.halt = None
        self.consecutive_failures = 0
        self.restarts_in_a_row = 0
        self.done = 0
        self.started = time.time()
        self.in_flight = {}

    def report(self, **extra):
        with self.lock:
            hc.count(self.manifest)
            rate = (time.time() - self.started) / max(1, self.done)
            current = sorted(self.in_flight)
            text = self.manifest["items"][self.in_flight[current[0]]]["text"][:120] if current else ""
            status(self.manifest, current=", ".join(current), text=text, rate=rate,
                   done_this_run=self.done, concurrency=CONCURRENCY, **extra)

    def ensure_server(self):
        """Restart the server if it is down; one thread does it, the rest wait."""
        with self.server_lock:
            if hc.healthy():
                return
            with self.lock:
                self.restarts_in_a_row += 1
                restarts = self.restarts_in_a_row
            if restarts > 3:
                self.halt = "sunucu üst üste 3 kez yeniden başlatılamadı"
                return
            say("sunucu yanıt vermiyor; yeniden başlatıyorum")
            time.sleep(5 * 3 ** (restarts - 1))
            try:
                hc.start_server(log_dir=os.path.join(JOB, "logs"), say=say, concurrency=CONCURRENCY)
            except RuntimeError as start_error:
                say(str(start_error)[-500:])


def process(run, index, item, ref_local, ref_text):
    """Generate one line: retry with a new seed while the clip fails validation;
    server trouble retries the same seed and does not use up an attempt."""
    config, max_attempts = run.config, run.config["max_attempts"]
    error, server_errors = None, 0
    while item["attempts"] < max_attempts and not run.halt:
        seed = config["base_seed"] + index + 1000 * item["attempts"]
        tmp = os.path.join(TMP, item["id"] + ".wav")
        t0 = time.time()
        try:
            wav = hc.synth(item["text"], config, seed, ref_local, ref_text, timeout=hc.read_timeout(item["text"]))
            with open(tmp, "wb") as f:
                f.write(wav)
            problem = hc.validate(tmp, item["text"], config)
            if problem:
                raise ValueError(problem)
            duration = round(hc.wav_duration(tmp), 2)
            hc.copy_atomic(tmp, os.path.join(JOB, item["file"]))
            gen_s = round(time.time() - t0, 2)
            with run.lock:
                item["attempts"] += 1
                item.update(status="done", seed=seed, bytes=len(wav), duration_s=duration,
                            gen_s=gen_s, error=None, finished_at=hc.now())
                hc.append_event(JOB, {"id": item["id"], "attempt": item["attempts"], "seed": seed,
                                      "status": "done", "bytes": len(wav), "duration_s": duration, "gen_s": gen_s})
                run.consecutive_failures, run.restarts_in_a_row = 0, 0
                run.done += 1
            return
        except hc.RequestRejected as rejected:  # a settings problem, the same for every line
            run.halt = f"sunucu isteği reddetti, ayarları kontrol et: {rejected}"
            return
        except ValueError as invalid:  # the clip came back but looks wrong: next seed
            error = str(invalid)
            with run.lock:
                item["attempts"] += 1
                say(f"{item['id']} deneme {item['attempts']}: {error}")
                hc.append_event(JOB, {"id": item["id"], "attempt": item["attempts"], "seed": seed,
                                      "status": "attempt_failed", "error": error})
        except Exception as failure:  # noqa: BLE001 - server/network/Drive trouble
            error = f"{type(failure).__name__}: {failure}"[:500]
            server_errors += 1
            say(f"{item['id']} sunucu/ağ hatası {server_errors}: {error}")
            run.ensure_server()
            if server_errors % 3 == 0:  # bounded: a line that keeps breaking the server still ends
                with run.lock:
                    item["attempts"] += 1
                    hc.append_event(JOB, {"id": item["id"], "attempt": item["attempts"], "seed": seed,
                                          "status": "attempt_failed", "error": error})
        finally:
            if os.path.exists(tmp):
                os.remove(tmp)
    if run.halt:
        return
    with run.lock:
        item.update(status="failed", error=error, finished_at=hc.now())
        hc.append_event(JOB, {"id": item["id"], "attempt": item["attempts"], "status": "failed", "error": error})
        run.consecutive_failures += 1
        if run.consecutive_failures >= 10:
            run.halt = "üst üste 10 satır başarısız; ayarları kontrol et"


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

    hc.start_server(log_dir=os.path.join(JOB, "logs"), say=say, concurrency=CONCURRENCY)
    say(f"başladı: {manifest['counts']}, aynı anda {CONCURRENCY} satır")
    run = Run(manifest, config)
    heartbeat()
    # The seed index comes from the line's id (1-based line number in the
    # frozen list), not its position here, so a job resumed from a subset of
    # the items still gives every line the seed it would have had.
    queue = iter([(int(item["id"]) - 1, item) for item in manifest["items"] if item["status"] == "pending"])
    futures, stopping, last_snapshot = {}, False, time.time()

    def submit_more(pool):
        while len(futures) < CONCURRENCY and not stopping and not run.halt:
            nxt = next(queue, None)
            if nxt is None:
                return
            index, item = nxt
            with run.lock:
                run.in_flight[item["id"]] = index
            futures[pool.submit(process, run, index, item, ref_local, ref_text)] = item["id"]

    with concurrent.futures.ThreadPoolExecutor(CONCURRENCY) as pool:
        submit_more(pool)
        while futures:
            finished, _ = concurrent.futures.wait(futures, timeout=5,
                                                  return_when=concurrent.futures.FIRST_COMPLETED)
            for future in finished:
                item_id = futures.pop(future)
                with run.lock:
                    run.in_flight.pop(item_id, None)
                try:
                    future.result()
                except Exception:  # noqa: BLE001 - a bug in one line must not lose the others
                    say(f"{item_id} beklenmeyen hata:\n" + traceback.format_exc())
            if not stopping and os.path.exists(STOP):
                stopping = True
                say("STOP dosyası bulundu; işteki satırlar bitince duruyorum")
            submit_more(pool)
            run.report()
            if time.time() - last_snapshot > 60:
                with run.lock:
                    hc.save_manifest(JOB, manifest)
                    hc.write_metadata(JOB, manifest)
                heartbeat()
                last_snapshot = time.time()

    with run.lock:
        counts = hc.count(manifest)
        hc.save_manifest(JOB, manifest, state="complete" if counts["pending"] == 0 else "halted")
        hc.write_metadata(JOB, manifest)
    run.report(finished=True, halted=run.halt)
    if run.halt:
        raise SystemExit(run.halt)
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
