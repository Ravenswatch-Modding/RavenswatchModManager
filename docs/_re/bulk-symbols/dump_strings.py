import json, os
from ghidra.program.model.data import StringDataInstance

out_dir = os.environ["RSMM_OUT"]
os.makedirs(out_dir, exist_ok=True)
listing = currentProgram.getListing()
strs = []
errs = 0
for d in listing.getDefinedData(True):
    if not d.hasStringValue():
        continue
    try:
        sdi = StringDataInstance.getStringDataInstance(d)
        v = sdi.getStringValue() if sdi is not None else None
        if v is None:
            v = d.getValue()
        if v is None:
            continue
        sv = str(v)
        if len(sv) < 3:
            continue
        strs.append({
            "addr": "0x%x" % int(d.getAddress().getOffset()),
            "value": sv,
            "length": len(sv),
            "type": str(d.getDataType().getName()),
        })
    except Exception as e:
        errs += 1
        if errs <= 5:
            print("[RSMM] string err:", repr(e))
out_path = os.path.join(out_dir, "strings.json")
with open(out_path, "w", encoding="utf-8") as f:
    json.dump(strs, f, indent=1, ensure_ascii=False)
print("[RSMM] strings: %d (errs=%d) -> %s" % (len(strs), errs, out_path))
