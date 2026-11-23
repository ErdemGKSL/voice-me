#@title 7. Okuma listesi: her satır bir cümle (yalnızca yeni işte)
#@markdown Drive'daki .txt dosyasının yolu (boşsa yükleme penceresi açılır):
LIST_DRIVE_PATH = "" #@param {type:"string"}
import unicodedata

if MODE == "continue":
    print("devam modu: liste Drive'daki işten okunuyor")
else:
    if LIST_DRIVE_PATH.strip():
        src = LIST_DRIVE_PATH if LIST_DRIVE_PATH.startswith("/") else os.path.join("/content/drive/MyDrive", LIST_DRIVE_PATH)
        raw_bytes = open(src, "rb").read()
    else:
        from google.colab import files
        uploaded = files.upload()
        assert uploaded, "dosya yüklenmedi"
        raw_bytes = next(iter(uploaded.values()))
    text = unicodedata.normalize("NFC", raw_bytes.decode("utf-8-sig"))
    items, seen = [], {}
    for line_no, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if not line or (CONFIG["skip_comment_lines"] and line.startswith("#")):
            continue
        bad = hc.unknown_tags(line)
        if bad:
            print(f"uyarı satır {line_no}: bilinmeyen etiket {bad}")
        if len(hc.strip_tags(line)) > 350:
            print(f"uyarı satır {line_no}: çok uzun ({len(line)} karakter), {CONFIG['max_new_tokens'] // 25} sn sınırını aşabilir")
        if line in seen:
            print(f"uyarı satır {line_no}: {seen[line]}. satırın tekrarı")
        seen.setdefault(line, line_no)
        n = len(items) + 1
        items.append({"id": f"{n:06d}", "line_no": line_no, "text": line, "status": "pending", "attempts": 0,
                      "seed": None, "file": f"wavs/{n:06d}.wav", "bytes": None, "duration_s": None,
                      "gen_s": None, "error": None, "finished_at": None})
    assert items, "listede okunacak satır yok"
    frozen = os.path.join(JOB_DIR, "reading_list.txt")
    with open(frozen, "w", encoding="utf-8") as f:
        f.write("\n".join(item["text"] for item in items) + "\n")
    hc.write_json_atomic(os.path.join(JOB_DIR, "config.json"), CONFIG)
    manifest = {
        "schema_version": hc.SCHEMA_VERSION, "job_name": JOB_NAME, "created_at": hc.now(), "updated_at": hc.now(),
        "state": "pending", "config": CONFIG, "config_sha256": hc.sha256_text(json.dumps(CONFIG, sort_keys=True)),
        "reference": REFERENCE, "reading_list": {"sha256": hc.sha256_file(frozen), "n_items": len(items)},
        "counts": {}, "items": items,
    }
    hc.save_manifest(JOB_DIR, manifest)
    est_h = sum(len(hc.strip_tags(i["text"])) for i in items) / 14 / 3600
    print(f"{len(items)} satır kaydedildi. Tahmini süre: ~{est_h:.1f} saat (L4'te ≈ gerçek zaman)")
