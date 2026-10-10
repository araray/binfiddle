#!/usr/bin/env python3
"""Independent CPU reference for the GLM EXL3 lab (Python 3.10+, stdlib).

Uses a literal MSB-first bit stream and a forward lane permutation, then f64
Walsh-Hadamard butterflies. Binfiddle uses u32 extraction, an inverse permutation,
and f32 matrix multiplication. This is a format check, not GPU runtime replay.
Reference: turboderp-org/exllamav3 @ 151539c77abc7ab7425d30da7a4e8e3c5c154e7b:
exl3_lib/quantize.py, quant/{pack.cu,codebook.cuh}, LinearEXL3.get_weight_tensor.
"""

import argparse
import json
import math
import struct
import subprocess
from pathlib import Path


def half(bits):
    return struct.unpack("<e", struct.pack("<H", bits))[0]


def codebook(window):
    x = (((window * 0xCBAC1FED) & 0xFFFFFFFF) & 0x8FFF8FFF) ^ 0x3B603B60
    # __hadd in the reference code rounds its sum to fp16.
    return struct.unpack("<e", struct.pack("<e", half(x >> 16) + half(x & 65535)))[0]


def fwht(values):
    values = list(values)
    step = 1
    while step < len(values):
        for base in range(0, len(values), step * 2):
            for j in range(step):
                a, b = values[base + j], values[base + j + step]
                values[base + j], values[base + j + step] = a + b, a - b
        step *= 2
    return [v / math.sqrt(len(values)) for v in values]


class Projection:
    def __init__(self, path, name):
        if path.stat().st_size > 16 * 1024 * 1024:
            raise ValueError("reference expects the small lab specimen, not a full shard")
        raw = path.read_bytes()
        size = struct.unpack_from("<Q", raw)[0]
        header = json.loads(raw[8:8 + size])
        fields = {}
        for suffix in ["trellis", "mcg", "suh", "svh"]:
            descriptor = header[f"{name}.{suffix}"]
            start, end = descriptor["data_offsets"]
            fields[suffix] = raw[8 + size + start:8 + size + end]
        if struct.unpack("<I", fields["mcg"])[0] != 0xCBAC1FED:
            raise ValueError("only the mcg codebook is supported")
        shape = header[f"{name}.trellis"]["shape"]
        self.bits = shape[2] // 16
        if shape[2] != self.bits * 16 or not 1 <= self.bits <= 8:
            raise ValueError("only integer EXL3 bitrates 1..8 are supported")
        self.ng = shape[1]
        self.su = [v[0] for v in struct.iter_unpack("<e", fields["suh"])]
        self.sv = [v[0] for v in struct.iter_unpack("<e", fields["svh"])]
        self.data = fields["trellis"]
        self.tiles = {}

    def tile(self, kg, ng):
        key = kg, ng
        if key not in self.tiles:
            byte_count = 32 * self.bits
            start = (kg * self.ng + ng) * byte_count
            words = struct.iter_unpack("<I", self.data[start:start + byte_count])
            stream = "".join(f"{word:032b}" for word, in words)
            wrapped = stream[-16:] + stream
            values = [codebook(int(wrapped[(t + 1) * self.bits:
                                          (t + 1) * self.bits + 16], 2))
                      for t in range(256)]
            # Forward permutation from encoded lane order to [k,n].
            matrix = [[0.0] * 16 for _ in range(16)]
            for lane in range(32):
                r, c = (lane % 4) * 2, lane // 4
                coordinates = [(r, c), (r + 1, c), (r + 8, c), (r + 9, c),
                               (r, c + 8), (r + 1, c + 8),
                               (r + 8, c + 8), (r + 9, c + 8)]
                for j, (k, n) in enumerate(coordinates):
                    matrix[k][n] = values[lane * 8 + j]
            self.tiles[key] = matrix
        return self.tiles[key]

    def wq(self, n, k):
        return self.tile(k // 16, n // 16)[k % 16][n % 16]

    def block(self, n0, k0):
        if (n0 % 128 or k0 % 128 or min(n0, k0) < 0
                or n0 + 128 > len(self.sv) or k0 + 128 > len(self.su)):
            raise ValueError("origin must name a complete, aligned 128x128 block")
        rows = [fwht([self.wq(n0 + n, k0 + k) for k in range(128)])
                for n in range(128)]
        columns = [fwht(column) for column in zip(*rows)]
        return [[columns[k][n] * self.sv[n0 + n] * self.su[k0 + k]
                 for k in range(128)] for n in range(128)]


def statistics(block):
    values = [v for row in block for v in row]
    mean = math.fsum(values) / len(values)
    return {"mean": mean,
            "variance": math.fsum((v - mean) ** 2 for v in values) / len(values),
            "absmax": max(map(abs, values))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sample", type=Path)
    parser.add_argument("--projection", choices=["up", "gate", "down"], default="up")
    parser.add_argument("--layer", type=int, default=12)
    parser.add_argument("--expert", type=int, default=27)
    parser.add_argument("--origin", default="0,0")
    parser.add_argument("--binfiddle", type=Path)
    parser.add_argument("--catalog", type=Path)
    parser.add_argument("--compare", type=Path, help="edited specimen: summarize logical differences")
    args = parser.parse_args()
    if bool(args.binfiddle) != bool(args.catalog):
        parser.error("--binfiddle and --catalog must be supplied together")
    name = (f"model.language_model.layers.{args.layer}.mlp.experts.{args.expert}."
            f"{args.projection}_proj")
    p = Projection(args.sample, name)
    n0, k0 = map(int, args.origin.split(","))
    block = p.block(n0, k0)
    probes = [(0, 0), (3, 5), (0, 16), (16, 0), (127, 127),
              (128, 128), (len(p.sv) - 1, len(p.su) - 1)]
    result = {"projection": name, "origin": [n0, k0], "block": statistics(block),
              "wq": [{"index": [n, k], "value": p.wq(n, k)} for n, k in probes]}
    if args.binfiddle:
        def run(command, flag, index):
            raw = subprocess.check_output([
                str(args.binfiddle.resolve()), "nn", "exl3", command,
                "--catalog", str(args.catalog.resolve()), "--trellis", name + ".trellis",
                flag, index, "--report-format", "json"], text=True)
            return json.loads(raw)["semantic"]
        for probe in result["wq"]:
            actual = run("decode", "--index", ",".join(map(str, probe["index"])))
            assert float(actual["value"]) == probe["value"], (actual, probe)
        actual = run("block", "--origin", args.origin)
        for key, expected in result["block"].items():
            assert math.isclose(float(actual[key]), expected, rel_tol=5e-5, abs_tol=1e-8), (key, actual, expected)
        result["binfiddle_check"] = "7 exact Wq probes; block statistics within f32 tolerance"
    if args.compare:
        after = Projection(args.compare, name).block(n0, k0)
        delta = [[after[n][k] - block[n][k] for k in range(128)] for n in range(128)]
        result["comparison"] = {
            "after": statistics(after), "delta": statistics(delta),
            "changed_values": sum(v != 0 for row in delta for v in row),
            "changed_columns": [k0 + k for k in range(128) if any(row[k] != 0 for row in delta)],
        }
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
