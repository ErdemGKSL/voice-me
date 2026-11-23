"""Upload a large local file to a Colab runtime in chunks.

    python3 upload.py <runtime url> <token> <local file> <remote path, e.g. content/x.zip>

Uses the Jupyter contents API the runtime exposes (`PUT /api/contents/...`,
base64, `chunk` 1, 2, ..., -1 for the last piece). The parent directory
must exist. Each piece is retried a few times; a failed upload is simply
re-run from the start.
"""
import base64
import json
import os
import sys
import time
import urllib.request

url, token, local, remote = sys.argv[1:5]
CHUNK = 40 * 1024 * 1024
size = os.path.getsize(local)
count = max(1, -(-size // CHUNK))


def put(body):
    for attempt in range(5):
        request = urllib.request.Request(
            f"{url}/api/contents/{remote}", data=json.dumps(body).encode(), method="PUT",
            headers={"X-Colab-Runtime-Proxy-Token": token, "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=300) as response:
                return response.status
        except Exception as error:  # noqa: BLE001 - network hiccups: retry
            print(f"  retry {attempt + 1}: {error}", flush=True)
            time.sleep(5 * (attempt + 1))
    raise SystemExit("upload failed")


with open(local, "rb") as f:
    for n in range(1, count + 1):
        data = f.read(CHUNK)
        body = {"type": "file", "format": "base64", "content": base64.b64encode(data).decode()}
        if count > 1:
            body["chunk"] = -1 if n == count else n
        put(body)
        print(f"{n}/{count}", flush=True)
print("uploaded", remote, size)
