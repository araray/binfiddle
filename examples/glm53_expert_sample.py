#!/usr/bin/env python3
"""Fetch one authentic EXL3 expert, not a runnable model. Python 3.10+, curl 8.4+.

Companion to docs/NN_DEEPDIVE_GLM_LAB.md. The pinned release, strict HTTP Range
checks, size limits, and acquisition manifest make the specimen reproducible.
No model code, credentials, Hugging Face client, or GPU is needed.
"""

import argparse
import hashlib
import functools
import json
import math
import re
import struct
import subprocess
import tempfile
from pathlib import Path


REPO = "Mia-AiLab/GLM-5.3-Flash-EXL3-4bpw-TensorFold"
REVISION = "76c0b5173166d2795dd48860f45d8224817f894c"
BASE = f"https://huggingface.co/{REPO}/resolve/{REVISION}"
MIB = 1024 * 1024


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def fetch_once(name, limit, span=None, attempt=0, *, scratch):
    """curl caps even unknown-length transfers; accept only exact 206 ranges.

    curl 8.4+ enforces --max-filesize during transfer as well as on advertised
    Content-Length. A server ignoring Range cannot cause a whole-shard download.
    """
    url = f"{BASE}/{name}"
    if span is not None:
        start, end = span  # half-open offsets throughout the manifest
        if not 0 <= start < end or end - start > limit:
            raise ValueError("range exceeds read budget")
        url += f"?binfiddle_range={start}-{end - 1}"
    else:
        url += "?download=true"
    if attempt:
        url += f"&attempt={attempt}"
    with tempfile.TemporaryDirectory(prefix=".http-", dir=scratch) as temporary:
        headers_path, body_path = Path(temporary) / "headers", Path(temporary) / "body"
        command = ["curl", "-q", "--fail", "--location", "--silent", "--show-error",
                   "--proto", "=https", "--proto-redir", "=https", "--max-time", "60",
                   "--max-filesize", str(limit), "--header", "Accept-Encoding: identity",
                   "--dump-header", str(headers_path), "--output", str(body_path)]
        if span is not None:
            command += ["--range", f"{start}-{end - 1}"]
        subprocess.run(command + [url], check=True, stdout=subprocess.DEVNULL,
                       stderr=subprocess.PIPE)
        blocks = [b for b in headers_path.read_text().split("\n\n") if b.startswith("HTTP/")]
        lines = blocks[-1].splitlines()
        status = int(lines[0].split()[1])
        headers = dict((key.lower(), value.strip()) for key, value in
                       (line.split(":", 1) for line in lines[1:] if ":" in line))
        total = None
        if headers.get("content-encoding", "identity") != "identity":
            raise ValueError("unexpected compressed response")
        if span is not None:
            match = re.fullmatch(r"bytes (\d+)-(\d+)/(\d+)",
                                 headers.get("content-range", ""))
            if status != 206 or match is None:
                raise ValueError("server did not honor Range; refusing full-shard download")
            first, last, total = map(int, match.groups())
            if (first, last + 1) != span or total < end:
                raise ValueError("server returned a different byte range")
            limit = end - start
        elif status != 200:
            raise ValueError(f"unexpected HTTP status {status}")
        length = headers.get("content-length")
        if length is not None and int(length) > limit:
            raise ValueError("response exceeds read budget")
        with body_path.open("rb") as body:
            data = body.read(limit + 1)
        if len(data) > limit or (span is not None and len(data) != limit):
            raise ValueError("oversized or truncated response")
        if length is not None and len(data) != int(length):
            raise ValueError("response body does not match Content-Length")
    return data, total


def fetch(name, limit, span=None, *, scratch):
    if span is not None and span[1] - span[0] > MIB:
        if span[1] - span[0] > limit:
            raise ValueError("range exceeds read budget")
        parts, totals = [], set()
        for start in range(span[0], span[1], MIB):
            data, total = fetch(name, MIB, (start, min(start + MIB, span[1])), scratch=scratch)
            parts.append(data)
            totals.add(total)
        if len(totals) != 1:
            raise ValueError("shard size changed between range chunks")
        return b"".join(parts), totals.pop()
    for attempt in range(3):
        try:
            return fetch_once(name, limit, span, attempt, scratch=scratch)
        except (OSError, ValueError, subprocess.CalledProcessError):
            if attempt == 2:
                raise


def acquire(output, layer, expert):
    # A failed run remains visibly incomplete (no manifest). Use a fresh output
    # directory to retry; do not silently mix bytes from earlier acquisitions.
    output.mkdir(parents=True, exist_ok=False)
    download = functools.partial(fetch, scratch=output)
    metadata = {}
    transferred = 0
    for name, limit in [("config.json", MIB), ("model.safetensors.index.json", 24 * MIB)]:
        data, _ = download(name, limit)
        (output / name).write_bytes(data)
        metadata[name] = {"sha256": sha256(data), "bytes": len(data)}
        transferred += len(data)
    config = json.loads((output / "config.json").read_bytes())
    index = json.loads((output / "model.safetensors.index.json").read_bytes())
    text = config["text_config"]
    if config["quantization_config"] != {
        "quant_method": "exl3", "bits": 4, "codebook": "mcg",
        "head_bits": 16, "scope": "glm53_routed_experts_only",
    }:
        raise ValueError("unexpected quantization profile at the pinned revision")
    if text["hidden_size"] != 4096 or text["moe_intermediate_size"] != 2048:
        raise ValueError("unexpected projection dimensions")

    prefix = f"model.language_model.layers.{layer}.mlp.experts.{expert}"
    expected = {}
    for projection in ["down", "gate", "up"]:
        n, k = (4096, 2048) if projection == "down" else (2048, 4096)
        for suffix, dtype, shape in [
            ("trellis", "I16", [k // 16, n // 16, 64]),
            ("mcg", "I32", [1]), ("suh", "F16", [k]), ("svh", "F16", [n]),
        ]:
            expected[f"{prefix}.{projection}_proj.{suffix}"] = (dtype, shape)
    weight_map = index["weight_map"]
    missing = set(expected) - weight_map.keys()
    if missing:
        raise ValueError(f"expert is absent or incomplete: {sorted(missing)}")
    shards = sorted({weight_map[name] for name in expected})
    if len(shards) > 3:
        raise ValueError("unexpectedly many shards for one expert")

    headers = {}
    shard_evidence = {}
    for shard in shards:
        if not re.fullmatch(r"model-\d{5}-of-\d{5}\.safetensors", shard):
            raise ValueError("unexpected shard filename")
        size_bytes, total = download(shard, 8, (0, 8))
        header_size = struct.unpack("<Q", size_bytes)[0]
        if not 2 <= header_size <= 4 * MIB or 8 + header_size > total:
            raise ValueError("invalid or oversized SafeTensors header")
        raw, same_total = download(shard, 4 * MIB, (8, 8 + header_size))
        if same_total != total:
            raise ValueError("shard size changed between requests")
        headers[shard] = json.loads(raw)
        (output / f"{shard}.header.json").write_bytes(raw)
        shard_evidence[shard] = {
            "file_bytes": total, "payload_base": 8 + header_size,
            "header_sha256": sha256(raw),
        }
        transferred += 8 + len(raw)

    records = []
    payloads = []
    sample_header = {"__metadata__": {
        "purpose": "binfiddle educational specimen; incomplete, not runnable",
        "source_repository": REPO, "source_revision": REVISION,
    }}
    cursor = 0
    for name, (dtype, shape) in sorted(expected.items()):
        shard = weight_map[name]
        descriptor = headers[shard][name]
        if descriptor["dtype"] != dtype or descriptor["shape"] != shape:
            raise ValueError(f"unexpected descriptor for {name}")
        start, end = descriptor["data_offsets"]
        size = math.prod(shape) * (4 if dtype == "I32" else 2)
        evidence = shard_evidence[shard]
        if not 0 <= start < end or end - start != size:
            raise ValueError(f"invalid payload extent for {name}")
        absolute = (evidence["payload_base"] + start, evidence["payload_base"] + end)
        if absolute[1] > evidence["file_bytes"] or cursor + size > 16 * MIB:
            raise ValueError("expert exceeds payload budget")
        data, total = download(shard, 8 * MIB, absolute)
        if total != evidence["file_bytes"]:
            raise ValueError("shard size changed during acquisition")
        if name.endswith(".mcg") and struct.unpack("<I", data)[0] != 0xCBAC1FED:
            raise ValueError("unsupported EXL3 codebook")
        sample_header[name] = {"dtype": dtype, "shape": shape,
                               "data_offsets": [cursor, cursor + size]}
        records.append({"name": name, "shard": shard, "dtype": dtype, "shape": shape,
                        "source_file_span": list(absolute), "sha256": sha256(data),
                        "sample_payload_span": [cursor, cursor + size]})
        payloads.append(data)
        cursor += size
        transferred += size
        print(f"{name}: {size:,} bytes", flush=True)

    header_bytes = json.dumps(sample_header, separators=(",", ":")).encode()
    header_bytes += b" " * (-len(header_bytes) % 8)
    sample = struct.pack("<Q", len(header_bytes)) + header_bytes + b"".join(payloads)
    sample_base = 8 + len(header_bytes)
    for record in records:
        record["sample_file_span"] = [sample_base + x for x in record["sample_payload_span"]]
    (output / "expert.safetensors").write_bytes(sample)
    manifest = {
        "schema": "binfiddle.glm-expert-acquisition/v1", "repository": REPO,
        "revision": REVISION, "layer": layer, "expert": expert,
        "coverage": "one reconstructed expert; not an original shard or runnable model",
        "metadata": metadata, "shards": shard_evidence, "fields": records,
        "sample": {"path": "expert.safetensors", "bytes": len(sample),
                   "payload_bytes": cursor, "sha256": sha256(sample)},
        "successful_response_body_bytes": transferred,
    }
    (output / "acquisition.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Saved {len(records)} fields, {cursor:,} payload bytes; "
          f"{transferred:,} bytes in completed responses (excluding retries).")
    print(f"Specimen: {output / 'expert.safetensors'}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="fresh output directory")
    parser.add_argument("--layer", type=int, default=12)
    parser.add_argument("--expert", type=int, default=27)
    args = parser.parse_args()
    version = subprocess.check_output(["curl", "-q", "--version"], text=True)
    match = re.match(r"curl (\d+)\.(\d+)", version)
    if match is None or tuple(map(int, match.groups())) < (8, 4):
        parser.error("curl 8.4+ is required to bound unknown-length downloads")
    if not 3 <= args.layer <= 45 or not 0 <= args.expert < 288:
        parser.error("this release has routed expert layers 3..45 and experts 0..287")
    acquire(args.output, args.layer, args.expert)


if __name__ == "__main__":
    main()
