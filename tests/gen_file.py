"""Writes a deterministic pseudo-random file: gen_file.py <seed> <MiB> <path>.

Used by CI so that two machines can check a transfer end to end without
exchanging anything but the transfer itself.
"""
import hashlib
import sys

seed, mib, path = sys.argv[1], int(sys.argv[2]), sys.argv[3]
block = hashlib.sha256(seed.encode()).digest()
with open(path, "wb") as f:
    for i in range(mib * 1024 * 1024 // 32 // 1024):
        chunk = bytearray()
        for j in range(1024):
            block = hashlib.sha256(block).digest()
            chunk += block
        f.write(chunk)
