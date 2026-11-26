import re,sys
txt=open(sys.argv[1]).read().splitlines()
start=[i for i,l in enumerate(txt) if 'Table G.1 - Main Conversion Table' in l][0]
pat=re.compile(r'^\s*(\d+)\s+([01]{16})\s+([1-4])\s+([01]{16})\s+([1-4])\s+([01]{16})\s+([1-4])\s+([01]{16})\s+([1-4])')
rows=[]
for l in txt[start:]:
    m=pat.match(l)
    if m: rows.append(m.groups())
rows=rows[:344]
assert len(rows)==344
main=rows[:256]; sub=rows[256:]
assert [int(r[0]) for r in main]==list(range(256)), 'main order'
assert [int(r[0]) for r in sub]==list(range(88)), 'sub order'
def emit(name,tab):
    out=[f"pub const {name}: [[(u16, u8); 4]; {len(tab)}] = ["]
    for r in tab:
        cells=", ".join(f"(0x{int(r[1+2*s],2):04X}, {r[2+2*s]})" for s in range(4))
        out.append(f"    [{cells}], // {r[0]}")
    out.append("];")
    return "\n".join(out)
print("//! ECMA-267 Annex G, Tables G.1 and G.2: 8-to-16 code words and next states.")
print("//! Generated from the standard's text by `docs/holo/gen_efm.py`; do not edit by hand.")
print("//! Each entry is indexed [byte][state - 1] and holds (code word, next state 1..=4).")
print()
print(emit("MAIN",main)); print(); print(emit("SUBST",sub))
