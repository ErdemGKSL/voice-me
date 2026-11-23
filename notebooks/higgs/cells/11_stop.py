#@title 11. Durdur (o anki satır bitince durur; sonra 9. hücreyle devam edebilirsin)
#@markdown "Run all" bu hücreyi de çalıştırır; yanlışlıkla durmasın diye yalnızca bu kutu işaretliyken durdurur:
STOP_NOW = False #@param {type:"boolean"}
if STOP_NOW:
    open("/content/higgs/STOP", "w").close()
    print("durdurma isteği bırakıldı; o anki satır bitince durur")
else:
    print("durdurmak için STOP_NOW'u işaretleyip bu hücreyi tekrar çalıştır")
