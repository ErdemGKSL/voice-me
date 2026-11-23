import glob, os, re, sys, unicodedata
sys.path.insert(0, "/home/user/voice-me/piper-voices/g2p/tr-dizge/train")
from dizge_phonemizer import spell_numbers
D = sys.argv[1]
old = {unicodedata.normalize("NFC", l.strip()).lower() for l in open(sys.argv[2], encoding="utf-8-sig")}
seen, out, dropped = set(), [], {"dup": 0, "old": 0, "bad": 0}
bad_chars = re.compile(r"[%₺$€/()\"“”«»&@#*_<>\[\]{}|=+~^]|[0-9]:[0-9]|[0-9]-[0-9]|[A-Za-z]{0}\b[a-z]+[0-9]")
NUM = ("sıfır|bir|iki|üç|dört|beş|altı|yedi|sekiz|dokuz|on|yirmi|otuz|kırk|elli|altmış|yetmiş|seksen|"
       "doksan|yüz|bin|milyon|milyar|trilyon")
def tidy(t):
    t = re.sub(rf"\b({NUM})'(\w)", r"\1\2", t)
    t = re.sub(r"\s+", " ", t).strip()
    return re.sub(r" ([.,!?;:…])", r"\1", t)
per = {}
for f in sorted(glob.glob(os.path.join(D, "t[0-9][0-9].txt"))):
    n = 0
    for line in open(f, encoding="utf-8"):
        t = unicodedata.normalize("NFC", line.strip()).strip("\"' ")
        t = re.sub(r"\s+", " ", t)
        if not t:
            continue
        if bad_chars.search(t):
            dropped["bad"] += 1; print("BAD", os.path.basename(f), t); continue
        spelled = tidy(spell_numbers(t)) if re.search(r"[0-9]", t) else t
        key = spelled.lower()
        if key in seen: dropped["dup"] += 1; continue
        if key in old: dropped["old"] += 1; continue
        seen.add(key); out.append(spelled); n += 1
    per[os.path.basename(f)] = n
open(os.path.join(D, "divers.txt"), "w", encoding="utf-8").write("\n".join(out) + "\n")
print(per); print("toplam", len(out), dropped)
print("! :", sum(l.endswith("!") for l in out), "? :", sum(l.endswith("?") for l in out), ": iç:", sum(":" in l for l in out))
