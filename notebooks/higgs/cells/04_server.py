#@title 4. Model sunucusunu başlat (~2 dk; ilk seferde birkaç dk daha)
import sys
sys.path.insert(0, "/content/higgs")
import importlib, higgs_common as hc
importlib.reload(hc)
hc.start_server(log_dir=os.path.join(JOB_DIR, "logs"))
# One throwaway request absorbs the first-use kernel builds.
hc.synth("Merhaba.", {"temperature": 0.8, "top_k": 50, "max_new_tokens": 256}, seed=1)
print("ısınma tamam")
