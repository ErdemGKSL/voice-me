#@title 10. İlerleme (durdurmak üretimi durdurmaz; istediğin zaman tekrar çalıştır)
#@markdown Her yeni tamamlanan satırı dinlet:
PLAY_EACH = False #@param {type:"boolean"}
import time
from IPython.display import Audio, clear_output, display
status_path = "/content/higgs/status.json"
last_played = None
while True:
    try:
        st = json.load(open(status_path, encoding="utf-8"))
    except (OSError, ValueError):
        st = {}
    clear_output(wait=True)
    counts, total = st.get("counts", {}), st.get("total", 0)
    done = counts.get("done", 0)
    bar = "█" * int(30 * done / max(1, total))
    print(f"[{bar:<30}] {done}/{total}  başarısız: {counts.get('failed', 0)}")
    rate = st.get("rate")
    if rate and st.get("done_this_run"):
        left = counts.get("pending", 0) * rate
        print(f"satır başına ~{rate:.1f} sn, kalan ~{left / 3600:.1f} saat")
    if st.get("current"):
        ids = st["current"].split(", ")
        print(f"şimdi ({len(ids)} satır aynı anda): {ids[0]}…{ids[-1]}  ör. {st.get('text', '')}")
    print("\nson günlük satırları:")
    print(subprocess.run("tail -n 6 /content/higgs/worker.log", shell=True, capture_output=True, text=True).stdout)
    if PLAY_EACH and st.get("last_done") and st["last_done"] != last_played:
        last_played = st["last_done"]
        path = os.path.join(JOB_DIR, "wavs", last_played + ".wav")
        if os.path.exists(path):
            display(Audio(path))
    running = subprocess.run("pgrep -f '[h]iggs_worker.py'", shell=True, capture_output=True).returncode == 0
    if st.get("finished") or not running:
        print("üretim çalışmıyor." + (f" sebep: {st['halted']}" if st.get("halted") else ""), "11–12. hücrelere geç.")
        break
    time.sleep(10)
