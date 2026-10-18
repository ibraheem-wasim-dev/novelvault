import json, re, sys

mode = sys.argv[1]
payload = sys.stdin.read()

if mode == "list":
    rows = json.loads(re.search(r"\[.*\]", payload, re.S).group(0))
    print(rows[0]["id"])
elif mode.startswith("file:"):
    needle = mode.split(":", 1)[1]
    rows = json.loads(re.search(r"\[.*\]", payload, re.S).group(0))
    print(next(r["id"] for r in rows if needle in r["rel_path"]))
else:
    print(json.loads(re.search(r"\{.*\}", payload, re.S).group(0))["id"])
