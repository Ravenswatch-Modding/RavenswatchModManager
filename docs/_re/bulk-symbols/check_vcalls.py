from ghidra.app.decompiler import DecompInterface
from ghidra.util.task import ConsoleTaskMonitor

addrs = [int(x, 16) for x in getScriptArgs()]
decomp = DecompInterface()
decomp.openProgram(currentProgram)
mon = ConsoleTaskMonitor()
fm = currentProgram.getFunctionManager()
sp = currentProgram.getAddressFactory().getDefaultAddressSpace()

for a in addrs:
    fn = fm.getFunctionAt(sp.getAddress(a))
    if fn is None:
        print("=== 0x%x NOT FOUND ===" % a)
        continue
    res = decomp.decompileFunction(fn, 60, mon)
    c = res.getDecompiledFunction().getC() if res and res.decompileCompleted() else ""
    sig = fn.getSignature(False).getPrototypeString(False)
    print("=== 0x%x %s ===" % (a, fn.getName()))
    print("SIG:", sig)
    for ln in c.splitlines():
        if "vftable" in ln or "(**(code **" in ln:
            print("  ", ln.strip())
decomp.dispose()
