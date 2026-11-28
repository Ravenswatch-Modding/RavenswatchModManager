# -*- coding: utf-8 -*-
# GENERATED loader for tools/gen_ghidra_vtables.py. Reads its data JSON (same
# basename + .json) or the path given as the postScript arg. Three phases:
#   1. create a <Class>_vtbl struct per game vtable + stamp it on the vtable
#      data (names the slots),
#   2. create a <Class> object struct whose field 0 is vftable:<Class>_vtbl*,
#   3. retype param_1 (this) of each unambiguous slot function to <Class>* so
#      its `(**(code**)(*this+0xNN))` vcalls decompile as this->vftable->Slot().
# Idempotent. Run headless (project must be UNLOCKED):
#   analyzeHeadless <proj-dir> <proj> -process -noanalysis \
#     -scriptPath <dir> -postScript rsmm_vtables.py rsmm_vtables.json
import json
from ghidra.program.model.data import (StructureDataType, PointerDataType,
    CategoryPath, DataTypeConflictHandler)
from ghidra.program.model.symbol import SourceType
from ghidra.program.model.pcode import HighFunctionDBUtil
from ghidra.app.decompiler import DecompInterface
from ghidra.util.task import ConsoleTaskMonitor

args = getScriptArgs()
if args:
    path = args[0]
else:
    src = getSourceFile().getAbsolutePath()
    path = src[:-3] + ".json" if src.endswith(".py") else src + ".json"
data = json.load(open(path))
vtbls = data['vtbls']
objs = data.get('objs', [])
retypes = data.get('retypes', [])
ctors = data.get('ctors', [])
ret_self = data.get('ret_self', [])

dtm = currentProgram.getDataTypeManager()
fm = currentProgram.getFunctionManager()
base = currentProgram.getImageBase()
pref = base.getNewAddress(0x140000000)
delta = base.getOffset() - pref.getOffset()
cat = CategoryPath('/rsmm_vtables')
ptr = PointerDataType()


def get_or_make_struct(sname):
    st = dtm.getDataType(cat, sname)
    return st, (st is None)


# Phase 1: vtbl structs + stamp on vtable data.
made = stamped = 0
for vt_addr, sname, fields in vtbls:
    st, is_new = get_or_make_struct(sname)
    if is_new:
        st = StructureDataType(cat, sname, 0)
        for fn, va in fields:
            st.add(ptr, 8, str(fn), '0x%x' % va)
        st = dtm.addDataType(st, DataTypeConflictHandler.REPLACE_HANDLER)
        made += 1
    a = base.getNewAddress(vt_addr + delta)
    try:
        clearListing(a, a.add(st.getLength() - 1))
        createData(a, st)
        stamped += 1
    except Exception as e:
        print('skip vtbl 0x%x: %s' % (vt_addr, e))
print('phase1 vtbl structs: %d created, %d stamped' % (made, stamped))

# Phase 2: object-layout structs {vftable: <Class>_vtbl*} + cache <Class>* ptr.
obj_ptr = {}
omade = 0
for obj_name, vtbl_name, obj_size in objs:
    ost, is_new = get_or_make_struct(obj_name)
    if is_new:
        vst = dtm.getDataType(cat, vtbl_name)
        # Size the struct past the largest observed field offset: Ghidra scales
        # pointer arithmetic by pointee size, so an 8-byte object turns a real
        # `+0x288` byte offset into a misleading `+0x51`.
        ost = StructureDataType(cat, obj_name, obj_size)
        vt_ptr = PointerDataType(vst) if vst is not None else ptr
        ost.replaceAtOffset(0, vt_ptr, 8, 'vftable', '')
        ost = dtm.addDataType(ost, DataTypeConflictHandler.REPLACE_HANDLER)
        omade += 1
    obj_ptr[obj_name] = PointerDataType(ost)
print('phase2 object structs: %d created' % omade)

# Phases 3-5 (one pass). Slot funcs carry NO committed formal params — auto-
# analysis leaves the decompiler's this/args uncommitted, so getParameter(0) is
# None. The WRONG fix (used until 2026-07-18, and it damaged a real project) is
# updateFunction(..., DYNAMIC_STORAGE_FORMAL_PARAMS, [this]): that commits
# EXACTLY ONE parameter and locks storage, DELETING every other argument —
# `f(longlong param_1, longlong *param_2)` became `f(<Class> *this)` with
# param_2 demoted to a bare `in_RDX` and all call arguments erased.
#
# Correct fix: decompile the function, commit ALL of the decompiler's derived
# parameters via HighFunctionDBUtil.commitParamsToDatabase, and only THEN
# retype parameter 0 (plus, for ctors/return-this funcs, the return). Nothing
# is invented: a function whose decompiled form has no parameters is skipped.
#
# The three target sets overlap (ret_self is a subset of retypes, and ~800
# ctors are in it too), so they are merged and each function is decompiled once
# — ~13k decompiles, several minutes, vs 17.6k if run separately.
targets = {}
for func_va, obj_name in retypes:
    targets[func_va] = [obj_name, False]
for func_va, obj_name in list(ctors) + list(ret_self):
    t = targets.get(func_va)
    if t is None:
        targets[func_va] = [obj_name, True]
    else:
        t[1] = True          # same class, but now the RETURN is known too

dec = DecompInterface()
dec.openProgram(currentProgram)
mon = ConsoleTaskMonitor()
RET_COMMIT = HighFunctionDBUtil.ReturnCommitOption.NO_COMMIT

typed = ret_typed = missing = noparam = nodecomp = badp0 = failed = 0
for func_va in sorted(targets):
    obj_name, want_ret = targets[func_va]
    func = fm.getFunctionAt(base.getNewAddress(func_va + delta))
    if func is None:
        missing += 1
        continue
    objptr = obj_ptr[obj_name]
    try:
        res = dec.decompileFunction(func, 60, mon)
        if not res.decompileCompleted():
            nodecomp += 1
            continue
        hf = res.getHighFunction()
        if hf is None:
            nodecomp += 1
            continue
        # commit the decompiler's OWN parameter list first — this is the whole
        # point: keep param_2, param_3, ... instead of truncating to one.
        HighFunctionDBUtil.commitParamsToDatabase(hf, True, RET_COMMIT,
                                                 SourceType.USER_DEFINED)
        p0 = func.getParameter(0)
        if p0 is None:
            noparam += 1        # genuinely no object param — do NOT invent one
            continue
        if p0.getDataType().getLength() != 8:
            badp0 += 1          # first arg isn't pointer-sized; not a `this`
            continue
        p0.setDataType(objptr, SourceType.USER_DEFINED)
        typed += 1
        if want_ret:
            func.setReturnType(objptr, SourceType.USER_DEFINED)
            ret_typed += 1
    except Exception as e:
        failed += 1
        if failed <= 20:
            print('skip 0x%x: %s' % (func_va, e))
print('phases3-5: %d this-typed (%d also return-typed), %d no-func, '
      '%d no-params, %d non-ptr-param0, %d decompile-fail, %d failed'
      % (typed, ret_typed, missing, noparam, badp0, nodecomp, failed))
