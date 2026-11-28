from collections import Counter
from ghidra.app.util.importer import MessageLog
from ghidra.feature.fid.analyzer import FidAnalyzer
from ghidra.feature.fid.db import FidFileManager
from ghidra.program.model.symbol import SourceType
from ghidra.util.task import ConsoleTaskMonitor

fm = currentProgram.getFunctionManager()

def count_sources():
    c = Counter()
    fun_cnt = 0
    total = 0
    for fn in fm.getFunctions(True):
        if fn.isExternal() or fn.isThunk():
            continue
        total += 1
        c[str(fn.getSymbol().getSource())] += 1
        if fn.getName().startswith("FUN_"):
            fun_cnt += 1
    return total, fun_cnt, dict(c)

tot0, fun0, src0 = count_sources()
print("[FID] before: total=%d FUN_=%d sources=%s" % (tot0, fun0, src0))

mgr = FidFileManager.getInstance()
mgr.load()
lang = currentProgram.getLanguage()
fids = []
for ff in mgr.getFidFiles():
    if ff.canProcessLanguage(lang):
        if not ff.isActive():
            ff.setActive(True)
        fids.append((str(ff.getName()), bool(ff.isActive())))
print("[FID] active x64 DBs:", fids)

analyzer = FidAnalyzer()
log = MessageLog()
mon = ConsoleTaskMonitor()
exe_set = currentProgram.getMemory().getExecuteSet()
ok = analyzer.added(currentProgram, exe_set, mon, log)
print("[FID] analyzer.added returned:", ok)
if str(log):
    print("[FID] log:", str(log)[:500])

tot1, fun1, src1 = count_sources()
print("[FID] after:  total=%d FUN_=%d sources=%s (delta_FUN=%d)" % (tot1, fun1, src1, fun1 - fun0))
