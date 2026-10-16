"""Check repository docs/assets using only Python's standard library.

This is not a provider, native runtime, Mermaid, or full JSON Schema test.
"""
from pathlib import Path
from urllib.parse import unquote, urlsplit
import datetime
import json
import math
import re
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
errors = []
checked_links = 0

for path in ROOT.rglob("*.md"):
    if ".git" in path.parts:
        continue
    content = path.read_text(encoding="utf-8")
    targets = re.findall(r"!?\[[^\]]*\]\(([^\s)]+)\)", content)
    targets += re.findall(r'(?:src|href)="([^"]+)"', content)
    for target in targets:
        url = urlsplit(target)
        if url.scheme or url.netloc or not url.path:
            continue
        checked_links += 1
        local = (path.parent / unquote(url.path)).resolve()
        if not local.is_relative_to(ROOT) or not local.exists():
            errors.append(f"{path.relative_to(ROOT)}: missing/unsafe local target {target}")
        elif url.fragment and local.suffix == ".md":
            headings = re.findall(r"^#{1,6}\s+(.+)$", local.read_text(encoding="utf-8"), re.M)
            slugs = [re.sub(r"[^\w\- ]", "", h.lower()).replace(" ", "-") for h in headings]
            if unquote(url.fragment) not in slugs:
                errors.append(f"{path.relative_to(ROOT)}: missing heading {target}")

json_files = list(ROOT.rglob("*.json"))
for path in json_files:
    try:
        json.loads(path.read_text(encoding="utf-8"))
    except (ValueError, UnicodeError) as exc:
        errors.append(f"{path.relative_to(ROOT)}: {exc}")

svgs = list((ROOT / "assets").glob("*.svg"))
for path in svgs:
    try:
        root = ET.parse(path).getroot()
        if root.tag != "{http://www.w3.org/2000/svg}svg":
            errors.append(f"{path.name}: not an SVG root")
    except ET.ParseError as exc:
        errors.append(f"{path.name}: {exc}")

try:
    schema = json.loads((ROOT / "schemas/observation.schema.json").read_text())
    example = json.loads((ROOT / "examples/observation.json").read_text())
    if set(example) != set(schema["required"]):
        errors.append("Observation example does not match required top-level keys")
    if example["schema_version"] != schema["properties"]["schema_version"]["const"]:
        errors.append("Observation schema version mismatch")
    datetime.datetime.fromisoformat(example["captured_at"].replace("Z", "+00:00"))
    if not re.fullmatch(r"[0-9a-f]{64}", example["image"]["sha256"]):
        errors.append("Observation hash must have 64 hexadecimal characters")
    for axis, dimension in (("x", "width"), ("y", "height")):
        if example["pointer"] and not 0 <= example["pointer"][axis] < example["scope"][dimension]:
            errors.append(f"Pointer {axis} lies outside example crop")
        expected = example["scope"][dimension] * example["transform"][f"scale_{axis}"]
        if not math.isclose(expected, example["image"][dimension]):
            errors.append(f"Image {dimension} does not match example transform")
except (ValueError, KeyError, TypeError, OSError) as exc:
    errors.append(f"Observation example consistency: {exc}")

if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print(f"OK: {checked_links} local links, {len(json_files)} JSON files, {len(svgs)} SVG assets; observation example consistency checked.")
print("Not checked: remote URLs, Mermaid rendering, full JSON Schema conformance, provider or native runtime behavior.")
