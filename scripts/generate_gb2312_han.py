"""Write the GB2312 Han characters (the 6,763 common simplified characters) used by
the core to keep traditional and variant characters (喲, 來) out of context-model
promotion. Deterministic: the codec table, in code-point order, one line."""
import pathlib

chars = sorted({c for hi in range(0xB0, 0xF8) for lo in range(0xA1, 0xFF)
                for c in [bytes([hi, lo]).decode("gb2312", errors="ignore")]
                if c and "一" <= c <= "鿿"})
out = pathlib.Path(__file__).resolve().parents[1] / "native/core/agent-ime-core/resources/gb2312_han.txt"
out.write_text("".join(chars) + "\n", encoding="utf-8")
print(f"gb2312_han={len(chars)} -> {out}")
