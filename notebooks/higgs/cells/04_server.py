#@title 4. Model sunucusunu başlat (~2 dk; ilk seferde birkaç dk daha)
#@markdown Aynı anda kaç satır üretilsin. L4'te ölçülen (satır başına): 1 → 4,2 sn, 8 → 0,68 sn, 16 → 0,41 sn, 32 → 0,25 sn. Ses aynı kalır. 64 L4'te belleğe sığmadı; bellek hatası görürsen düşür.
CONCURRENCY = 32 #@param [1, 4, 8, 16, 32] {type:"raw"}
import sys
sys.path.insert(0, "/content/higgs")
import importlib, higgs_common as hc
importlib.reload(hc)
hc.start_server(log_dir=os.path.join(JOB_DIR, "logs"), concurrency=CONCURRENCY)
# One throwaway request absorbs the first-use kernel builds.
hc.synth("Merhaba.", {"temperature": 0.8, "top_k": 50, "max_new_tokens": 256}, seed=1)
print("ısınma tamam")
