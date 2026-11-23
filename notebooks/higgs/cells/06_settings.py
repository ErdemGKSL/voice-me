#@title 6. Model ayarları (yalnızca yeni işte)
#@markdown Dil: model dili metinden anlar; Türkçe için `auto` ya da `tr` aynıdır. Yalnızca en, zh, fr, de, it, ja, ko, pt, ru, es sunucuya ayrıca bildirilir (sunucu başka dil adı kabul etmiyor).
LANGUAGE = "auto" #@param ["auto", "tr", "en", "de", "fr", "es", "it", "pt", "ru", "ja", "ko", "zh"]
TEMPERATURE = 0.8 #@param {type:"slider", min:0.1, max:1.5, step:0.05}
TOP_K = 50 #@param {type:"integer"}
#@markdown 0 = kullanma:
TOP_P = 0 #@param {type:"number"}
REPETITION_PENALTY = 0 #@param {type:"number"}
#@markdown 1024 ≈ en fazla 40 sn ses. Uzun satırlar için artır:
MAX_NEW_TOKENS = 1024 #@param {type:"integer"}
#@markdown Her satırın seed'i = BASE_SEED + satır sırası (tekrar üretilebilir):
BASE_SEED = 42 #@param {type:"integer"}
#@markdown Her satırın başına eklenecek etiketler, ör. `<|prosody:speed_slow|>` (boş = yok):
PREFIX_TAGS = "" #@param {type:"string"}
#@markdown Doğrulamadan geçemeyen satır kaç kez (farklı seed'le) denensin:
MAX_ATTEMPTS = 4 #@param {type:"integer"}
#@markdown `#` ile başlayan satırları atla:
SKIP_COMMENT_LINES = True #@param {type:"boolean"}

print("Kullanılabilir etiketler:")
for category, tags in hc.KNOWN_TAGS.items():
    print(f"  {category}: " + ", ".join(f"<|{category}:{t}|>" for t in sorted(tags)))
if MODE == "continue":
    print("\ndevam modu: ayarlar Drive'daki işten okunuyor; bu formdaki değerler kullanılmaz")
else:
    assert not hc.unknown_tags(PREFIX_TAGS), f"bilinmeyen etiket: {hc.unknown_tags(PREFIX_TAGS)}"
    CONFIG = {
        "model": hc.MODEL, "sglang_omni": hc.SGLANG_OMNI, "language": LANGUAGE,
        "temperature": TEMPERATURE, "top_k": TOP_K, "top_p": TOP_P or None,
        "repetition_penalty": REPETITION_PENALTY or None, "max_new_tokens": MAX_NEW_TOKENS,
        "base_seed": BASE_SEED, "prefix_tags": PREFIX_TAGS, "max_attempts": MAX_ATTEMPTS,
        "seed_policy": "base_seed + index + 1000 * attempt", "skip_comment_lines": SKIP_COMMENT_LINES,
    }
    print("\nayarlar:", json.dumps(CONFIG, ensure_ascii=False))
