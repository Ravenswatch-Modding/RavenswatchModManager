import json
from ghidra.app.decompiler import DecompInterface
from ghidra.util.task import ConsoleTaskMonitor

data = json.load(open(getScriptArgs()[0], encoding="utf-8"))
decomp = DecompInterface()
decomp.openProgram(currentProgram)
mon = ConsoleTaskMonitor()
fm = currentProgram.getFunctionManager()
sp = currentProgram.getAddressFactory().getDefaultAddressSpace()

for e in data["entries"]:
    a = int(e["addr"], 16)
    fn = fm.getFunctionAt(sp.getAddress(a))
    if fn is None:
        print("MISSING:", e["addr"])
        continue
    res = decomp.decompileFunction(fn, 30, mon)
    c = res.getDecompiledFunction().getC() if res and res.decompileCompleted() else ""
    first_lines = " | ".join(ln.strip() for ln in c.splitlines() if ln.strip())[:160]
    print("CHECK:", e["addr"], "expected=", e["name"])
    print("  getName(True)=", fn.getName(True), "ns=", fn.getParentNamespace().getName(True))
    print("  decompile=", first_lines)
decomp.dispose()
