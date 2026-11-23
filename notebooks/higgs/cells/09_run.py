#@title 9. Üretimi başlat (arka planda çalışır; bu hücre hemen biter)
#@markdown Devam ederken daha önce başarısız olan satırlar da yeniden denensin:
RETRY_FAILED = False #@param {type:"boolean"}
import socket, subprocess, time
lock = os.path.join(JOB_DIR, "worker.lock")
if os.path.exists(lock):
    held = json.load(open(lock))
    # A lock left by a crashed worker on this same machine is stale at once.
    crashed_here = held.get("host") == socket.gethostname() and not os.path.exists(f"/proc/{held.get('pid')}")
    if not crashed_here and time.time() - held.get("heartbeat", 0) < 120:
        raise SystemExit("Bu iş şu an başka bir yerde çalışıyor gibi (son sinyal 2 dk'dan yeni). "
                         "Aynı işi iki makinede birden çalıştırma.")
if RETRY_FAILED:
    manifest = hc.load_manifest(JOB_DIR)
    for item in manifest["items"]:
        if item["status"] == "failed":
            hc.append_event(JOB_DIR, {"id": item["id"], "status": "reset"})
if os.path.exists("/content/higgs/STOP"):
    os.remove("/content/higgs/STOP")
if subprocess.run("pgrep -f '[h]iggs_worker.py'", shell=True, capture_output=True).returncode == 0:
    print("çalışan bir üretim zaten var; 10. hücreden izle")
else:
    subprocess.Popen(["python3", "/content/higgs/higgs_worker.py", JOB_DIR],
                     stdout=open("/content/higgs/worker.out", "ab"), stderr=subprocess.STDOUT, start_new_session=True)
    print("başladı. 10. hücreden izle; tarayıcıyı kapatsan da makine açık kaldıkça devam eder.")
